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
        "reasoning" | "think" | "thought" => "thinking",
        other => other,
    };
    if mapped == "thinking" || STD_ROLES.contains(&mapped) || extra.iter().any(|e| e.to_lowercase() == mapped) {
        Some(mapped.to_string())
    } else {
        None
    }
}

/// Removes markdown decoration in front of a line: heading marks, quotes and bullets.
fn strip_md_prefix(line: &str) -> &str {
    let mut t = line.trim_start();
    loop {
        let before = t;
        t = t.trim_start_matches('#').trim_start();
        t = t.strip_prefix('>').unwrap_or(t).trim_start();
        if let Some(r) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            t = r.trim_start();
        }
        if t == before {
            return t;
        }
    }
}

fn is_marker(c: char) -> bool {
    c == '*' || c == '_' || c == '`'
}

/// `label: rest` -> (label, rest). Tolerates markdown such as `**User:** hi`, `**User**: hi`, `- User: hi`.
/// Labels have no whitespace and are at most 32 bytes.
fn split_label(line: &str) -> Option<(String, String)> {
    let t = strip_md_prefix(line);
    let idx = t.find(':')?;
    let raw = &t[..idx];
    let opener: String = raw.chars().take_while(|c| is_marker(*c)).collect();
    let mut rest = &t[idx + 1..];
    if !opener.is_empty() {
        if let Some(r) = rest.strip_prefix(opener.as_str()) {
            rest = r;
        }
    }
    let label = raw.trim_matches(is_marker);
    if label.is_empty() || label.len() > 32 || label.contains(char::is_whitespace) {
        return None;
    }
    Some((label.to_string(), rest.strip_prefix(' ').unwrap_or(rest).to_string()))
}

/// A markdown heading or a fully bold line that is only a label: `## User`, `**Assistant**`.
fn heading_label(line: &str) -> Option<String> {
    let t = line.trim();
    let is_heading = t.starts_with('#');
    let is_bold = t.len() > 4 && (t.starts_with("**") && t.ends_with("**"));
    if !is_heading && !is_bold {
        return None;
    }
    let inner = strip_md_prefix(t).trim().trim_matches(is_marker).trim().trim_end_matches(':').trim();
    if inner.is_empty() || inner.len() > 32 || inner.contains(char::is_whitespace) {
        None
    } else {
        Some(inner.to_string())
    }
}

fn is_sep(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3 && t.chars().all(|c| c == '-')
}


/// Turns (role, text) pairs into messages. A "thinking" block is attached to the next assistant message.
pub fn pairs_to_messages(g: Vec<(String, String)>, warnings: &mut Vec<String>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    for (r, c) in g {
        if r == "thinking" {
            pending.push(c);
            continue;
        }
        let mut m = vec![("role", Value::str(r.clone())), ("content", Value::str(c))];
        if r == "assistant" && !pending.is_empty() {
            m.insert(1, ("thinking", Value::str(pending.join("\n\n"))));
            pending.clear();
        }
        out.push(Value::obj(m));
    }
    if !pending.is_empty() {
        warnings.push("a Thinking block had no Assistant message after it and was dropped".to_string());
    }
    out
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
            if let Some(role) = canon_role(&label, extra) {
                cur.push((role, rest));
                continue;
            }
        }
        if let Some(role) = heading_label(line).and_then(|h| canon_role(&h, extra)) {
            cur.push((role, String::new()));
            continue;
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
                cur.push((name.clone(), rest));
                continue;
            }
        }
        if let Some(h) = heading_label(line) {
            if let Some(name) = fields.iter().find(|f| f.to_lowercase() == h.to_lowercase()) {
                let starts_new = (fields.first() == Some(name) && !cur.is_empty()) || cur.iter().any(|(k, _)| k == name);
                if starts_new {
                    finish(std::mem::take(&mut cur), &mut out);
                }
                cur.push((name.clone(), String::new()));
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
            let msgs = pairs_to_messages(g.clone(), &mut warnings);
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

pub(crate) fn count_labels(text: &str, mode_fields: &[String], conversation: bool, extra: &[String]) -> usize {
    text.lines()
        .filter(|l| {
            let by_colon = split_label(l).map(|(lab, _)| if conversation { canon_role(&lab, extra).is_some() } else { mode_fields.iter().any(|f| f.to_lowercase() == lab.to_lowercase()) }).unwrap_or(false);
            by_colon || heading_label(l).map(|h| if conversation { canon_role(&h, extra).is_some() } else { mode_fields.iter().any(|f| f.to_lowercase() == h.to_lowercase()) }).unwrap_or(false)
        })
        .count()
}

/// op "text_records": records from a .txt / .md file.
/// mode "conversation" | "fields" need role/field labels (markdown tolerated) and report a warning when none exist;
/// mode "plain" makes one {"text": ...} record per line (split "lines") or per paragraph (split "paragraphs").
pub fn text_records(v: &Value) -> Result<Value, String> {
    let text = v.get("text").and_then(|t| t.as_str()).ok_or("missing string field 'text'")?;
    let mode = v.get("mode").and_then(|m| m.as_str()).unwrap_or("plain");
    let fields = str_list(v, "fields");
    let extra = str_list(v, "extra_roles");
    let mut warnings: Vec<String> = Vec::new();
    let records: Vec<Value> = if mode == "plain" {
        let paragraphs = v.get("split").and_then(|m| m.as_str()) == Some("paragraphs");
        let parts: Vec<String> = if paragraphs {
            let mut out = Vec::new();
            let mut cur: Vec<&str> = Vec::new();
            for line in text.lines() {
                if line.trim().is_empty() {
                    if !cur.is_empty() {
                        out.push(cur.join("\n"));
                        cur.clear();
                    }
                } else {
                    cur.push(line);
                }
            }
            if !cur.is_empty() {
                out.push(cur.join("\n"));
            }
            out
        } else {
            text.lines().filter(|l| !l.trim().is_empty()).map(|l| l.to_string()).collect()
        };
        parts.into_iter().map(|p| Value::obj(vec![("text", Value::str(p))])).collect()
    } else {
        let conversation = mode == "conversation";
        if !conversation && fields.is_empty() {
            return Err("fields mode needs a non-empty 'fields' list".into());
        }
        if count_labels(text, &fields, conversation, &extra) == 0 {
            warnings.push(if conversation {
                "no role labels found (use lines like \"User: ...\" and \"Assistant: ...\", or markdown headings such as \"## User\")".to_string()
            } else {
                format!("no field labels found (expected lines starting with {})", fields.join(", "))
            });
            Vec::new()
        } else {
            let (groups, w) = if conversation { parse_conversations(text, &extra) } else { parse_fields(text, &fields) };
            warnings.extend(w);
            let mut conv_warn: Vec<String> = Vec::new();
            let recs: Vec<Value> = groups
                .into_iter()
                .map(|g| {
                    if conversation {
                        let msgs = pairs_to_messages(g, &mut conv_warn);
                        Value::obj(vec![("messages", Value::Arr(msgs))])
                    } else {
                        Value::Obj(g.into_iter().map(|(k, c)| (k, Value::str(c))).collect())
                    }
                })
                .collect();
            warnings.extend(conv_warn);
            recs
        }
    };
    Ok(Value::obj(vec![("records", Value::Arr(records)), ("warnings", Value::Arr(warnings.into_iter().map(Value::str).collect()))]))
}

/// Splits a document into pieces of at most `max` characters, on paragraph boundaries (long paragraphs on whitespace).
pub fn doc_chunks(text: &str, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut push_piece = |cur: &mut String, out: &mut Vec<String>| {
        if !cur.trim().is_empty() {
            out.push(cur.trim().to_string());
        }
        cur.clear();
    };
    for para in text.split("\n\n") {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }
        let mut rest = para;
        while rest.chars().count() > max {
            let mut cut = rest.char_indices().nth(max).map(|(i, _)| i).unwrap_or(rest.len());
            if let Some(ws) = rest[..cut].rfind(char::is_whitespace) {
                if ws > cut / 2 {
                    cut = ws;
                }
            }
            if !cur.is_empty() {
                push_piece(&mut cur, &mut out);
            }
            out.push(rest[..cut].trim().to_string());
            rest = rest[cut..].trim_start();
        }
        if rest.is_empty() {
            continue;
        }
        if cur.chars().count() + rest.chars().count() + 2 > max {
            push_piece(&mut cur, &mut out);
        }
        if !cur.is_empty() {
            cur.push_str("\n\n");
        }
        cur.push_str(rest);
    }
    push_piece(&mut cur, &mut out);
    out
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
                if let Some(t) = m.get("thinking").and_then(|x| x.as_str()) {
                    lines.push(format!("Thinking: {}", t));
                }
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
    fn markdown_labels() {
        let r = run(r###"{"text":"## System\nBe brief.\n\n**User:** hello **world**\n- Assistant: hi\n**User**: again"}"###);
        let recs = r.get("records").unwrap().as_arr().unwrap();
        let m = recs[0].get("messages").unwrap().as_arr().unwrap();
        assert_eq!(m.len(), 4);
        assert_eq!(m[0].get("role").unwrap().as_str().unwrap(), "system");
        assert_eq!(m[0].get("content").unwrap().as_str().unwrap(), "Be brief.");
        assert_eq!(m[1].get("content").unwrap().as_str().unwrap(), "hello **world**");
        assert_eq!(m[2].get("content").unwrap().as_str().unwrap(), "hi");
        assert_eq!(m[3].get("content").unwrap().as_str().unwrap(), "again");
    }

    #[test]
    fn text_records_modes() {
        let go = |j: &str| text_records(&parse(j).unwrap()).unwrap();
        let r = go(r#"{"mode":"plain","split":"paragraphs","text":"a\nb\n\nc"}"#);
        let recs = r.get("records").unwrap().as_arr().unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].get("text").unwrap().as_str().unwrap(), "a\nb");
        let r = go(r#"{"mode":"plain","split":"lines","text":"a\n\nb"}"#);
        assert_eq!(r.get("records").unwrap().as_arr().unwrap().len(), 2);
        let r = go(r#"{"mode":"conversation","text":"just some prose"}"#);
        assert_eq!(r.get("records").unwrap().as_arr().unwrap().len(), 0);
        assert_eq!(r.get("warnings").unwrap().as_arr().unwrap().len(), 1);
        let r = go(r#"{"mode":"fields","fields":["q","a"],"text":"q: x\na: y"}"#);
        assert_eq!(r.get("records").unwrap().as_arr().unwrap().len(), 1);
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
