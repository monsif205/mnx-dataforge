//! Plain-text authoring format <-> records.
//!
//! Conversation mode:   "User: hello" / "Assistant: hi" lines, `---` separates conversations.
//! Fields mode:         "question: ..." / "answer: ..." lines; the first field starts a new record.
//! Lines without a label continue the previous message, so a message can span several lines.

use crate::json::Value;
use crate::text::{estimate_tokens, record_text};

const STD_ROLES: [&str; 4] = ["system", "user", "assistant", "tool"];

fn canon_role(label: &str, extra: &[String]) -> Option<String> {
    let l = label.trim().to_lowercase();
    let mapped = match l.as_str() {
        "human" => "user",
        "ai" | "gpt" | "bot" | "model" => "assistant",
        other => other,
    };
    if STD_ROLES.contains(&mapped) || extra.iter().any(|e| e.to_lowercase() == mapped) {
        Some(mapped.to_string())
    } else {
        None
    }
}

/// `label: rest` -> (label, rest). Labels have no spaces and are at most 32 bytes.
fn split_label(line: &str) -> Option<(&str, &str)> {
    let t = line.trim_start();
    let idx = t.find(':')?;
    let label = &t[..idx];
    if label.is_empty() || label.len() > 32 || label.contains(char::is_whitespace) {
        return None;
    }
    let rest = &t[idx + 1..];
    Some((label, rest.strip_prefix(' ').unwrap_or(rest)))
}

fn is_sep(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3 && t.chars().all(|c| c == '-')
}

type Pairs = Vec<(String, String)>;

fn finish(mut cur: Pairs, out: &mut Vec<Pairs>) {
    if cur.is_empty() {
        return;
    }
    for p in cur.iter_mut() {
        p.1 = p.1.trim().to_string();
    }
    out.push(cur);
}

pub fn parse_conversations(text: &str, extra: &[String]) -> (Vec<Pairs>, Vec<String>) {
    let mut out: Vec<Pairs> = Vec::new();
    let mut warn: Vec<String> = Vec::new();
    let mut cur: Pairs = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if is_sep(line) {
            finish(std::mem::take(&mut cur), &mut out);
            continue;
        }
        if let Some((label, rest)) = split_label(line) {
            if let Some(role) = canon_role(label, extra) {
                cur.push((role, rest.to_string()));
                continue;
            }
        }
        if let Some(last) = cur.last_mut() {
            last.1.push('\n');
            last.1.push_str(line);
        } else if !line.trim().is_empty() {
            warn.push(format!("line {}: text before any role label was read as a user message", i + 1));
            cur.push(("user".to_string(), line.to_string()));
        }
    }
    finish(cur, &mut out);
    (out, warn)
}

pub fn parse_fields(text: &str, fields: &[String]) -> (Vec<Pairs>, Vec<String>) {
    let mut out: Vec<Pairs> = Vec::new();
    let mut warn: Vec<String> = Vec::new();
    let mut cur: Pairs = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if is_sep(line) {
            finish(std::mem::take(&mut cur), &mut out);
            continue;
        }
        if let Some((label, rest)) = split_label(line) {
            if let Some(name) = fields.iter().find(|f| f.to_lowercase() == label.to_lowercase()) {
                let starts_new = (fields.first() == Some(name) && !cur.is_empty()) || cur.iter().any(|(k, _)| k == name);
                if starts_new {
                    finish(std::mem::take(&mut cur), &mut out);
                }
                cur.push((name.clone(), rest.to_string()));
                continue;
            }
        }
        if let Some(last) = cur.last_mut() {
            last.1.push('\n');
            last.1.push_str(line);
        } else if !line.trim().is_empty() {
            warn.push(format!("line {}: text before any field label was ignored", i + 1));
        }
    }
    finish(cur, &mut out);
    (out, warn)
}

fn str_list(v: &Value, k: &str) -> Vec<String> {
    v.get(k).and_then(|a| a.as_arr()).map(|a| a.iter().filter_map(|s| s.as_str().map(|t| t.to_string())).collect()).unwrap_or_default()
}

/// op "script": text -> records (+ warnings, token estimates).
pub fn build(v: &Value) -> Result<Value, String> {
    let text = v.get("text").and_then(|t| t.as_str()).ok_or("missing string field 'text'")?;
    let mode = v.get("mode").and_then(|m| m.as_str()).unwrap_or("conversation");
    let (groups, mut warnings) = if mode == "fields" {
        let fields = str_list(v, "fields");
        if fields.is_empty() {
            return Err("fields mode needs a non-empty 'fields' list".into());
        }
        parse_fields(text, &fields)
    } else {
        parse_conversations(text, &str_list(v, "extra_roles"))
    };
    let mut records: Vec<Value> = Vec::new();
    let mut tokens: Vec<Value> = Vec::new();
    let mut total_tokens = 0usize;
    let mut messages = 0usize;
    for (n, g) in groups.iter().enumerate() {
        let rec = if mode == "fields" {
            Value::Obj(g.iter().map(|(k, val)| (k.clone(), Value::str(val.clone()))).collect())
        } else {
            messages += g.len();
            for (k, val) in g {
                if val.is_empty() {
                    warnings.push(format!("record {}: empty {} message", n + 1, k));
                }
            }
            let msgs: Vec<Value> = g.iter().map(|(r, c)| Value::obj(vec![("role", Value::str(r.clone())), ("content", Value::str(c.clone()))])).collect();
            Value::obj(vec![("messages", Value::Arr(msgs))])
        };
        let t = estimate_tokens(&record_text(&rec));
        total_tokens += t;
        tokens.push(Value::int(t as i64));
        records.push(rec);
    }
    Ok(Value::obj(vec![
        ("records", Value::Arr(records)),
        ("warnings", Value::Arr(warnings.into_iter().map(Value::str).collect())),
        ("tokens", Value::Arr(tokens)),
        ("total_tokens", Value::int(total_tokens as i64)),
        ("messages", Value::int(messages as i64)),
    ]))
}

/// op "to_script": records -> text (inverse of `build`).
pub fn render(v: &Value) -> Result<Value, String> {
    let recs = v.get("records").and_then(|r| r.as_arr()).ok_or("missing array field 'records'")?;
    let mode = v.get("mode").and_then(|m| m.as_str()).unwrap_or("conversation");
    let fields = str_list(v, "fields");
    let mut parts: Vec<String> = Vec::new();
    for r in recs {
        let mut lines: Vec<String> = Vec::new();
        if mode == "fields" {
            for f in &fields {
                if let Some(Value::Str(s)) = r.get(f) {
                    lines.push(format!("{}: {}", f, s));
                }
            }
        } else if let Some(msgs) = r.get("messages").and_then(|m| m.as_arr()) {
            for m in msgs {
                let role = m.get("role").and_then(|x| x.as_str()).unwrap_or("user");
                let content = m.get("content").and_then(|x| x.as_str()).unwrap_or("");
                let mut label = role.to_string();
                if let Some(c) = label.get_mut(0..1) {
                    c.make_ascii_uppercase();
                }
                lines.push(format!("{}: {}", label, content));
            }
        }
        parts.push(lines.join("\n"));
    }
    Ok(Value::obj(vec![("text", Value::str(parts.join("\n---\n")))]))
}

/// op "tokens": estimates for a list of texts.
pub fn tokens(v: &Value) -> Result<Value, String> {
    let texts = v.get("texts").and_then(|t| t.as_arr()).ok_or("missing array field 'texts'")?;
    let counts: Vec<usize> = texts.iter().map(|t| estimate_tokens(t.as_str().unwrap_or(""))).collect();
    let total: usize = counts.iter().sum();
    Ok(Value::obj(vec![
        ("counts", Value::Arr(counts.iter().map(|c| Value::int(*c as i64)).collect())),
        ("total", Value::int(total as i64)),
        ("avg", Value::float(if counts.is_empty() { 0.0 } else { total as f64 / counts.len() as f64 })),
        ("max", Value::int(counts.iter().copied().max().unwrap_or(0) as i64)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    fn run(input: &str) -> Value {
        build(&parse(input).unwrap()).unwrap()
    }

    #[test]
    fn conversation_with_arabic_and_multiline() {
        let r = run(r#"{"text":"System: كن مختصرا\nUser: ما عاصمة المغرب؟\nمع التفاصيل\nAssistant: الرباط"}"#);
        let recs = r.get("records").unwrap().as_arr().unwrap();
        assert_eq!(recs.len(), 1);
        let msgs = recs[0].get("messages").unwrap().as_arr().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[1].get("content").unwrap().as_str().unwrap(), "ما عاصمة المغرب؟\nمع التفاصيل");
        assert_eq!(msgs[2].get("role").unwrap().as_str().unwrap(), "assistant");
        assert!(r.get("total_tokens").unwrap().as_f64().unwrap() > 0.0);
    }

    #[test]
    fn separators_aliases_and_warnings() {
        let r = run(r#"{"text":"stray\nHuman: a\nAI: b\n---\nuser: c\nassistant:"}"#);
        let recs = r.get("records").unwrap().as_arr().unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].get("messages").unwrap().as_arr().unwrap().len(), 3);
        let w = r.get("warnings").unwrap().as_arr().unwrap();
        assert!(w.iter().any(|x| x.as_str().unwrap().contains("before any role")));
        assert!(w.iter().any(|x| x.as_str().unwrap().contains("empty assistant")));
    }

    #[test]
    fn fields_mode_starts_new_record_on_first_field() {
        let r = run(r#"{"mode":"fields","fields":["question","answer"],"text":"question: a?\nanswer: b\nquestion: c?\nanswer: d\nmore d"}"#);
        let recs = r.get("records").unwrap().as_arr().unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[1].get("answer").unwrap().as_str().unwrap(), "d\nmore d");
    }

    #[test]
    fn roundtrip_text_records_text() {
        let src = "User: سؤال\nAssistant: جواب\n---\nUser: x\nAssistant: y";
        let input = Value::obj(vec![("text", Value::str(src))]).to_json();
        let recs = run(&input).get("records").unwrap().clone();
        let back = render(&Value::obj(vec![("records", recs)])).unwrap();
        assert_eq!(back.get("text").unwrap().as_str().unwrap(), src);
    }

    #[test]
    fn token_estimates_are_sane() {
        let t = |s: &str| estimate_tokens(s);
        assert_eq!(t(""), 0);
        assert!(t("Hello world.") >= 2 && t("Hello world.") <= 4);
        assert!(t("مرحبا بكم في المغرب") >= 4);
        assert!(t(&"word ".repeat(100)) > t(&"word ".repeat(10)));
        assert!(t("日本語のテキスト") >= 6);
    }
}
