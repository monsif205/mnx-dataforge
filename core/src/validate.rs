//! Validation engine. Never modifies records. Every rule is labelled deterministic or heuristic.

use crate::dedup;
use crate::json::Value;
use crate::text::{fnv1a, normalize, record_text, shingles};
use std::collections::HashMap;
use std::time::Instant;

pub struct Config {
    pub kind: String, // "conversation" | "qa" | "generic"
    pub required: Vec<String>,
    pub extra_roles: Vec<String>,
    pub min_chars: usize,
    pub max_chars: usize,
    pub near_threshold: f64, // 0 disables near-duplicate check
    pub max_findings: usize,
    pub dedup: bool, // false = caller runs the dataset-wide `dedup` op separately
}

impl Config {
    pub fn from_value(v: &Value) -> Config {
        let strs = |k: &str| -> Vec<String> {
            v.get(k)
                .and_then(|x| x.as_arr())
                .map(|a| a.iter().filter_map(|s| s.as_str().map(|t| t.to_string())).collect())
                .unwrap_or_default()
        };
        Config {
            kind: v.get("kind").and_then(|x| x.as_str()).unwrap_or("generic").to_string(),
            required: strs("required"),
            extra_roles: strs("extra_roles"),
            min_chars: v.get("min_chars").and_then(|x| x.as_f64()).unwrap_or(0.0) as usize,
            max_chars: v.get("max_chars").and_then(|x| x.as_f64()).unwrap_or(0.0) as usize,
            near_threshold: v.get("near_threshold").and_then(|x| x.as_f64()).unwrap_or(0.85),
            max_findings: v.get("max_findings").and_then(|x| x.as_f64()).unwrap_or(5000.0) as usize,
            dedup: v.get("dedup").and_then(|x| x.as_bool()).unwrap_or(true),
        }
    }

    pub fn to_value(&self) -> Value {
        let arr = |a: &Vec<String>| Value::Arr(a.iter().map(|s| Value::str(s.clone())).collect());
        Value::obj(vec![
            ("kind", Value::str(self.kind.clone())),
            ("required", arr(&self.required)),
            ("extra_roles", arr(&self.extra_roles)),
            ("min_chars", Value::int(self.min_chars as i64)),
            ("max_chars", Value::int(self.max_chars as i64)),
            ("near_threshold", Value::float(self.near_threshold)),
            ("max_findings", Value::int(self.max_findings as i64)),
            ("dedup", Value::Bool(self.dedup)),
        ])
    }
}

pub struct Finding {
    pub record: String,
    pub rule: &'static str,
    pub severity: &'static str,
    pub deterministic: bool,
    pub message: String,
}

/// (rule id, deterministic, description)
const RULES: &[(&str, bool, &str)] = &[
    ("R001", true, "record is a JSON object"),
    ("R002", true, "required field present"),
    ("R003", true, "required field not empty"),
    ("R004", true, "exact duplicate (normalized text)"),
    ("R005", false, "near duplicate (shingle similarity)"),
    ("R006", false, "abnormal length"),
    ("R007", false, "repetitive language"),
    ("C001", true, "conversation has messages array"),
    ("C002", true, "message content not empty"),
    ("C003", true, "message role valid"),
    ("C004", false, "role sequence plausible"),
    ("C005", true, "tool message follows tool call"),
    ("C006", true, "no accidental repeated message"),
    ("C007", true, "tool_calls structure valid"),
    ("C008", true, "message shape valid"),
    ("C009", false, "conversation has at least two messages"),
];

fn rule_active(id: &str, kind: &str) -> bool {
    if id.starts_with('C') {
        kind == "conversation"
    } else {
        true
    }
}

struct Sink {
    findings: Vec<Finding>,
    cap: usize,
    truncated: bool,
    errors: usize,
    warnings: usize,
    infos: usize,
    per_rule: HashMap<&'static str, usize>,
}

impl Sink {
    fn push(&mut self, record: &str, rule: &'static str, severity: &'static str, det: bool, msg: String) {
        match severity {
            "error" => self.errors += 1,
            "warning" => self.warnings += 1,
            _ => self.infos += 1,
        }
        *self.per_rule.entry(rule).or_insert(0) += 1;
        if self.findings.len() < self.cap {
            self.findings.push(Finding { record: record.to_string(), rule, severity, deterministic: det, message: msg });
        } else {
            self.truncated = true;
        }
    }
}

fn check_tool_calls(id: &str, idx: usize, tc: &Value, s: &mut Sink) {
    let arr = match tc.as_arr() {
        Some(a) if !a.is_empty() => a,
        _ => {
            s.push(id, "C007", "error", true, format!("message {}: tool_calls must be a non-empty array", idx));
            return;
        }
    };
    for (k, call) in arr.iter().enumerate() {
        let name = call
            .get("name")
            .and_then(|n| n.as_str())
            .or_else(|| call.get_path("function.name").and_then(|n| n.as_str()));
        if name.map(|n| n.trim().is_empty()).unwrap_or(true) {
            s.push(id, "C007", "error", true, format!("message {}: tool call {} has no name", idx, k));
        }
        let args = call.get("arguments").or_else(|| call.get_path("function.arguments"));
        match args {
            Some(Value::Obj(_)) => {}
            Some(Value::Str(a)) => {
                if crate::json::parse(a).is_err() {
                    s.push(id, "C007", "error", true, format!("message {}: tool call {} arguments string is not valid JSON", idx, k));
                }
            }
            _ => s.push(id, "C007", "error", true, format!("message {}: tool call {} arguments missing or wrong type", idx, k)),
        }
    }
}

fn check_conversation(id: &str, rec: &Value, cfg: &Config, s: &mut Sink) {
    let msgs = match rec.get("messages").and_then(|m| m.as_arr()) {
        Some(m) => m,
        None => {
            s.push(id, "C001", "error", true, "missing 'messages' array".to_string());
            return;
        }
    };
    if msgs.len() < 2 {
        s.push(id, "C009", "warning", false, format!("only {} message(s)", msgs.len()));
    }
    let std_roles = ["system", "user", "assistant", "tool"];
    let mut prev_role: Option<String> = None;
    let mut prev_had_calls = false;
    let mut prev_content: Option<String> = None;
    let mut seen_non_system = false;
    for (i, m) in msgs.iter().enumerate() {
        if m.as_obj().is_none() {
            s.push(id, "C008", "error", true, format!("message {} is not an object", i));
            continue;
        }
        let role = match m.get("role").and_then(|r| r.as_str()) {
            Some(r) => r.to_string(),
            None => {
                s.push(id, "C003", "error", true, format!("message {} has no role", i));
                continue;
            }
        };
        let custom = cfg.extra_roles.iter().any(|r| *r == role);
        if !std_roles.contains(&role.as_str()) && !custom {
            s.push(id, "C003", "error", true, format!("message {} has unknown role '{}'", i, role));
        }
        let tool_calls = m.get("tool_calls");
        let content = match m.get("content") {
            Some(Value::Str(c)) => Some(c.clone()),
            Some(Value::Null) | None => None,
            Some(_) => {
                s.push(id, "C008", "error", true, format!("message {} content must be a string", i));
                None
            }
        };
        let empty = content.as_ref().map(|c| c.trim().is_empty()).unwrap_or(true);
        let has_calls = tool_calls.is_some();
        if empty && !(role == "assistant" && has_calls) {
            s.push(id, "C002", "error", true, format!("message {} ({}) has empty content", i, role));
        }
        if let Some(tc) = tool_calls {
            if role != "assistant" {
                s.push(id, "C007", "error", true, format!("message {}: tool_calls only valid on assistant messages", i));
            }
            check_tool_calls(id, i, tc, s);
        }
        if std_roles.contains(&role.as_str()) {
            if role == "system" && i > 0 {
                s.push(id, "C004", "warning", false, format!("message {}: system message is not first", i));
            }
            if role != "system" && !seen_non_system {
                seen_non_system = true;
                if role != "user" {
                    s.push(id, "C004", "warning", false, format!("message {}: first non-system message is '{}', expected 'user'", i, role));
                }
            }
            if let Some(p) = &prev_role {
                if (role == "user" || role == "assistant") && *p == role && !has_calls && !prev_had_calls {
                    s.push(id, "C004", "warning", false, format!("message {}: two consecutive '{}' messages", i, role));
                }
            }
            if role == "tool" {
                let ok = match &prev_role {
                    Some(p) => (p == "assistant" && prev_had_calls) || p == "tool",
                    None => false,
                };
                if !ok {
                    s.push(id, "C005", "error", true, format!("message {}: tool message does not follow an assistant tool call", i));
                }
            }
        }
        if let (Some(pr), Some(pc), Some(c)) = (&prev_role, &prev_content, &content) {
            if *pr == role && !c.trim().is_empty() && pc.trim() == c.trim() {
                s.push(id, "C006", "warning", true, format!("message {} repeats the previous message exactly", i));
            }
        }
        prev_role = Some(role);
        prev_had_calls = has_calls;
        prev_content = content;
    }
    if let Some(last) = &prev_role {
        if last != "assistant" {
            s.push(id, "C004", "info", false, "conversation does not end with an assistant message".to_string());
        }
    }
}

fn check_required(id: &str, rec: &Value, fields: &[String], s: &mut Sink) {
    for f in fields {
        match rec.get_path(f) {
            None | Some(Value::Null) => s.push(id, "R002", "error", true, format!("required field '{}' is missing", f)),
            Some(Value::Str(t)) if t.trim().is_empty() => s.push(id, "R003", "error", true, format!("required field '{}' is empty", f)),
            Some(Value::Arr(a)) if a.is_empty() => s.push(id, "R003", "error", true, format!("required field '{}' is empty", f)),
            _ => {}
        }
    }
}

pub fn run(records: &[(String, Value)], cfg: &Config) -> Value {
    let t0 = Instant::now();
    let mut s = Sink {
        findings: Vec::new(),
        cap: cfg.max_findings,
        truncated: false,
        errors: 0,
        warnings: 0,
        infos: 0,
        per_rule: HashMap::new(),
    };
    let required: Vec<String> = if !cfg.required.is_empty() {
        cfg.required.clone()
    } else if cfg.kind == "qa" {
        vec!["question".to_string(), "answer".to_string()]
    } else {
        Vec::new()
    };

    let mut norm_hashes: Vec<u64> = Vec::with_capacity(records.len());
    let mut shingle_sets: Vec<Vec<u64>> = Vec::with_capacity(records.len());
    let mut hash_ids: Vec<usize> = Vec::new(); // record indexes that have text

    for (idx, (id, rec)) in records.iter().enumerate() {
        if rec.as_obj().is_none() {
            s.push(id, "R001", "error", true, "record is not a JSON object".to_string());
            continue;
        }
        if cfg.kind == "conversation" {
            check_conversation(id, rec, cfg, &mut s);
        }
        check_required(id, rec, &required, &mut s);
        let text = record_text(rec);
        let norm = normalize(&text);
        let chars = text.chars().count();
        if cfg.min_chars > 0 && chars < cfg.min_chars {
            s.push(id, "R006", "warning", false, format!("very short: {} chars (min {})", chars, cfg.min_chars));
        }
        if cfg.max_chars > 0 && chars > cfg.max_chars {
            s.push(id, "R006", "warning", false, format!("very long: {} chars (max {})", chars, cfg.max_chars));
        }
        let words: Vec<&str> = norm.split(' ').filter(|w| !w.is_empty()).collect();
        if words.len() >= 20 {
            let mut uniq = words.clone();
            uniq.sort_unstable();
            uniq.dedup();
            if (uniq.len() as f64) / (words.len() as f64) < 0.25 {
                s.push(id, "R007", "warning", false, "text is highly repetitive (low unique-word ratio)".to_string());
            }
        }
        if !norm.is_empty() {
            hash_ids.push(idx);
            norm_hashes.push(fnv1a(norm.as_bytes()));
            shingle_sets.push(if cfg.dedup && cfg.near_threshold > 0.0 { shingles(&norm) } else { Vec::new() });
        }
    }

    // Exact duplicates (deterministic).
    let groups = if cfg.dedup { dedup::exact_groups(&norm_hashes) } else { Vec::new() };
    let mut exact_pair: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    for g in &groups {
        let first = &records[hash_ids[g[0]]].0;
        for &m in &g[1..] {
            let rid = &records[hash_ids[m]].0;
            s.push(rid, "R004", "warning", true, format!("exact duplicate of record {}", first));
            exact_pair.insert((g[0].min(m), g[0].max(m)));
        }
    }
    // Near duplicates (heuristic).
    if cfg.dedup && cfg.near_threshold > 0.0 {
        for (a, b, j) in dedup::near_pairs(&shingle_sets, cfg.near_threshold) {
            if exact_pair.contains(&(a, b)) || norm_hashes[a] == norm_hashes[b] {
                continue;
            }
            let rid = &records[hash_ids[b]].0;
            let other = &records[hash_ids[a]].0;
            s.push(rid, "R005", "warning", false, format!("near duplicate of record {} (similarity {:.2}, heuristic)", other, j));
        }
    }

    // Reproducible id: hash of config + record ids + record contents.
    let cfg_json = cfg.to_value().to_json();
    let mut buf: Vec<u8> = cfg_json.into_bytes();
    for (id, rec) in records {
        buf.extend_from_slice(id.as_bytes());
        buf.push(0);
        buf.extend_from_slice(rec.to_json().as_bytes());
        buf.push(1);
    }
    let validation_id = format!("val-{:016x}", fnv1a(&buf));

    let rules: Vec<Value> = RULES
        .iter()
        .filter(|(id, _, _)| rule_active(id, &cfg.kind))
        .map(|(id, det, desc)| {
            Value::obj(vec![
                ("id", Value::str(*id)),
                ("deterministic", Value::Bool(*det)),
                ("description", Value::str(*desc)),
                ("findings", Value::int(*s.per_rule.get(id).unwrap_or(&0) as i64)),
            ])
        })
        .collect();
    let failed = RULES.iter().filter(|(id, _, _)| rule_active(id, &cfg.kind) && s.per_rule.contains_key(id)).count();
    let active = RULES.iter().filter(|(id, _, _)| rule_active(id, &cfg.kind)).count();
    let findings: Vec<Value> = s
        .findings
        .iter()
        .map(|f| {
            Value::obj(vec![
                ("record", Value::str(f.record.clone())),
                ("rule", Value::str(f.rule)),
                ("severity", Value::str(f.severity)),
                ("deterministic", Value::Bool(f.deterministic)),
                ("message", Value::str(f.message.clone())),
            ])
        })
        .collect();

    Value::obj(vec![
        ("validation_id", Value::str(validation_id)),
        ("records_examined", Value::int(records.len() as i64)),
        ("errors", Value::int(s.errors as i64)),
        ("warnings", Value::int(s.warnings as i64)),
        ("infos", Value::int(s.infos as i64)),
        ("rules_executed", Value::int(active as i64)),
        ("rules_failed", Value::int(failed as i64)),
        ("rules_passed", Value::int((active - failed) as i64)),
        ("rules", Value::Arr(rules)),
        ("findings", Value::Arr(findings)),
        ("truncated", Value::Bool(s.truncated)),
        ("duration_ms", Value::int(t0.elapsed().as_millis() as i64)),
        ("config", cfg.to_value()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    fn recs(items: &[(&str, &str)]) -> Vec<(String, Value)> {
        items.iter().map(|(i, j)| (i.to_string(), parse(j).unwrap())).collect()
    }
    fn cfg(kind: &str) -> Config {
        Config::from_value(&parse(&format!(r#"{{"kind":"{}"}}"#, kind)).unwrap())
    }
    fn rules_of(rep: &Value, rec: &str) -> Vec<String> {
        rep.get("findings").unwrap().as_arr().unwrap().iter()
            .filter(|f| f.get("record").unwrap().as_str().unwrap() == rec)
            .map(|f| f.get("rule").unwrap().as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn good_conversation_is_clean() {
        let r = recs(&[("a", r#"{"messages":[{"role":"system","content":"s"},{"role":"user","content":"مرحبا"},{"role":"assistant","content":"أهلا"}]}"#)]);
        let rep = run(&r, &cfg("conversation"));
        assert_eq!(rep.get("errors").unwrap().as_f64().unwrap(), 0.0);
        assert_eq!(rep.get("warnings").unwrap().as_f64().unwrap(), 0.0);
    }

    #[test]
    fn bad_conversation_findings() {
        let r = recs(&[
            ("e", r#"{"messages":[{"role":"user","content":""},{"role":"robot","content":"x"}]}"#),
            ("t", r#"{"messages":[{"role":"user","content":"q"},{"role":"tool","content":"r"}]}"#),
            ("c", r#"{"messages":[{"role":"user","content":"q"},{"role":"assistant","tool_calls":[{"name":"f","arguments":"{bad"}]}]}"#),
            ("n", r#"{"foo":1}"#),
            ("o", r#"[1]"#),
        ]);
        let rep = run(&r, &cfg("conversation"));
        assert!(rules_of(&rep, "e").contains(&"C002".to_string()));
        assert!(rules_of(&rep, "e").contains(&"C003".to_string()));
        assert!(rules_of(&rep, "t").contains(&"C005".to_string()));
        assert!(rules_of(&rep, "c").contains(&"C007".to_string()));
        assert!(rules_of(&rep, "n").contains(&"C001".to_string()));
        assert!(rules_of(&rep, "o").contains(&"R001".to_string()));
    }

    #[test]
    fn duplicates_and_required() {
        let r = recs(&[
            ("1", r#"{"question":"ما عاصمة المغرب؟","answer":"الرباط"}"#),
            ("2", r#"{"question":"ما عاصمة المغرب","answer":"الرباط!"}"#),
            ("3", r#"{"question":"x","answer":""}"#),
            ("4", r#"{"question":"only q"}"#),
        ]);
        let rep = run(&r, &cfg("qa"));
        assert!(rules_of(&rep, "2").contains(&"R004".to_string()), "punctuation-insensitive exact dup");
        assert!(rules_of(&rep, "3").contains(&"R003".to_string()));
        assert!(rules_of(&rep, "4").contains(&"R002".to_string()));
    }

    #[test]
    fn reproducible_id_and_no_mutation() {
        let r = recs(&[("1", r#"{"question":"a b c d","answer":"e"}"#)]);
        let before = r[0].1.to_json();
        let a = run(&r, &cfg("qa"));
        let b = run(&r, &cfg("qa"));
        assert_eq!(a.get("validation_id"), b.get("validation_id"));
        assert_eq!(before, r[0].1.to_json());
    }
}
