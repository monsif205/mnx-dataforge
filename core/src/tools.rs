//! Small engine-side helpers used by the UI: pretty printing, list previews, the Enter-key role
//! logic of Quick Write, and the GitHub release parser used by the in-app updater.

use crate::json::{parse, Value};

fn s_of<'a>(v: &'a Value, k: &str) -> Result<&'a str, String> {
    v.get(k).and_then(|x| x.as_str()).ok_or_else(|| format!("missing string field '{}'", k))
}

fn list(v: &Value, k: &str) -> Vec<String> {
    v.get(k).and_then(|a| a.as_arr()).map(|a| a.iter().filter_map(|s| s.as_str().map(|t| t.to_string())).collect()).unwrap_or_default()
}

fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// ---------------------------------------------------------------- pretty

/// op "pretty": strict-parse a JSON text and return it indented. Parse errors are returned as messages.
pub fn pretty(v: &Value) -> Result<Value, String> {
    let text = s_of(v, "text")?;
    let indent = v.get("indent").and_then(|i| i.as_f64()).unwrap_or(2.0).clamp(0.0, 8.0) as usize;
    let parsed = parse(text).map_err(|e| e.to_string())?;
    Ok(Value::obj(vec![("text", Value::str(parsed.to_pretty(indent)))]))
}

// ---------------------------------------------------------------- preview

fn preview_one(data: &str, max: usize) -> String {
    let parsed = match parse(data) {
        Ok(p) => p,
        Err(_) => return take_chars(data, max),
    };
    if let Some(msgs) = parsed.get("messages").and_then(|m| m.as_arr()) {
        for m in msgs {
            if m.get("role").and_then(|r| r.as_str()) == Some("user") {
                return take_chars(m.get("content").and_then(|c| c.as_str()).unwrap_or(""), max);
            }
        }
        return take_chars(msgs.first().and_then(|m| m.get("content")).and_then(|c| c.as_str()).unwrap_or(""), max);
    }
    let mut out = String::new();
    if let Some(items) = parsed.as_obj() {
        for (_, val) in items {
            if out.chars().count() >= max {
                break;
            }
            if let Value::Str(s) = val {
                out.push_str(s);
                out.push_str(" · ");
            }
        }
    }
    take_chars(&out, max)
}

/// op "preview": one short line per record JSON text (conversation -> first user message).
pub fn preview(v: &Value) -> Result<Value, String> {
    let items = v.get("datas").and_then(|d| d.as_arr()).ok_or("missing array field 'datas'")?;
    let max = v.get("max").and_then(|m| m.as_f64()).unwrap_or(140.0) as usize;
    let out: Vec<Value> = items.iter().map(|d| Value::str(preview_one(d.as_str().unwrap_or(""), max))).collect();
    Ok(Value::obj(vec![("previews", Value::Arr(out))]))
}

// ---------------------------------------------------------------- Quick Write: Enter key

const CONV: [&str; 4] = ["System", "User", "Assistant", "Tool"];

fn labels_for(conversation: bool, fields: &[String]) -> Vec<String> {
    if conversation {
        CONV.iter().map(|s| s.to_string()).collect()
    } else {
        fields.to_vec()
    }
}

/// `label: rest` split; labels have no whitespace.
fn split_label(line: &str) -> Option<(&str, &str)> {
    let t = line.trim_start();
    let idx = t.find(':')?;
    let label = &t[..idx];
    if label.is_empty() || label.chars().any(|c| c.is_whitespace()) {
        return None;
    }
    Some((label, &t[idx + 1..]))
}

fn canon(label: &str, labels: &[String], conversation: bool) -> Option<String> {
    let low = label.to_lowercase();
    if let Some(h) = labels.iter().find(|l| l.to_lowercase() == low) {
        return Some(h.clone());
    }
    if conversation {
        match low.as_str() {
            "human" => return Some("User".into()),
            "ai" | "gpt" | "bot" | "model" => return Some("Assistant".into()),
            _ => {}
        }
    }
    None
}

fn last_label(before: &str, labels: &[String], conversation: bool) -> Option<String> {
    for line in before.lines().rev().chain(std::iter::once("")).take(before.lines().count() + 1) {
        if let Some((l, _)) = split_label(line) {
            if let Some(c) = canon(l, labels, conversation) {
                return Some(c);
            }
        }
    }
    None
}

fn next_label(cur: Option<&str>, conversation: bool, fields: &[String]) -> String {
    if conversation {
        return match cur {
            Some("User") => "Assistant",
            Some("Assistant") => "User",
            Some("System") => "User",
            Some("Tool") => "Assistant",
            _ => "User",
        }
        .to_string();
    }
    if fields.is_empty() {
        return String::new();
    }
    match cur.and_then(|c| fields.iter().position(|f| f == c)) {
        Some(i) => fields[(i + 1) % fields.len()].clone(),
        None => fields[0].clone(),
    }
}

/// op "enter": `before` is the text up to (not including) the newline the user just typed.
/// Returns what to insert after that newline, or `empty: true` when the current line is a bare
/// label (Enter must then do nothing, so no empty messages are created).
pub fn enter(v: &Value) -> Result<Value, String> {
    let before = s_of(v, "before")?;
    let conversation = v.get("mode").and_then(|m| m.as_str()).unwrap_or("conversation") != "fields";
    let fields = list(v, "fields");
    let labels = labels_for(conversation, &fields);
    let cur = last_label(before, &labels, conversation);
    let line = before.rsplit('\n').next().unwrap_or("");
    let bare = match split_label(line) {
        Some((l, rest)) => canon(l, &labels, conversation).is_some() && rest.trim().is_empty(),
        None => false,
    };
    let next = next_label(cur.as_deref(), conversation, &fields);
    let insert = if next.is_empty() { String::new() } else { format!("{}: ", next) };
    Ok(Value::obj(vec![
        ("empty", Value::Bool(bare)),
        ("insert", Value::str(insert)),
        ("current", cur.map(Value::str).unwrap_or(Value::Null)),
        ("next", Value::str(next)),
    ]))
}

// ---------------------------------------------------------------- updater

/// op "release_info": parse GitHub's "latest release" JSON. Tag format `build-<N>` (N = versionCode);
/// the APK is the first asset whose name ends with `.apk`.
pub fn release_info(v: &Value) -> Result<Value, String> {
    let rel = parse(s_of(v, "json")?).map_err(|e| format!("bad release JSON: {}", e))?;
    let current = v.get("current").and_then(|c| c.as_f64()).unwrap_or(0.0) as i64;
    let tag = rel.get("tag_name").and_then(|t| t.as_str()).ok_or("release has no tag_name")?;
    let code: i64 = tag.trim_start_matches("build-").trim_start_matches('v').parse().map_err(|_| format!("unrecognised tag '{}'", tag))?;
    let mut name = String::new();
    let mut url = String::new();
    let mut size = 0i64;
    if let Some(assets) = rel.get("assets").and_then(|a| a.as_arr()) {
        for a in assets {
            let n = a.get("name").and_then(|x| x.as_str()).unwrap_or("");
            if n.ends_with(".apk") {
                name = n.to_string();
                url = a.get("browser_download_url").and_then(|x| x.as_str()).unwrap_or("").to_string();
                size = a.get("size").and_then(|x| x.as_f64()).unwrap_or(0.0) as i64;
                break;
            }
        }
    }
    let notes = take_chars(rel.get("body").and_then(|b| b.as_str()).unwrap_or(""), 600);
    Ok(Value::obj(vec![
        ("available", Value::Bool(code > current && !url.is_empty())),
        ("version_code", Value::int(code)),
        ("name", Value::str(name)),
        ("url", Value::str(url)),
        ("size", Value::int(size)),
        ("notes", Value::str(notes)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(s: &str) -> Value {
        parse(s).unwrap()
    }

    #[test]
    fn pretty_and_error() {
        let r = pretty(&o(r#"{"text":"{\"a\":[1,2]}"}"#)).unwrap();
        assert!(r.get("text").unwrap().as_str().unwrap().contains("\n  \"a\": ["));
        assert!(pretty(&o(r#"{"text":"{bad"}"#)).is_err());
    }

    #[test]
    fn previews() {
        let r = preview(&o(r#"{"datas":["{\"messages\":[{\"role\":\"system\",\"content\":\"s\"},{\"role\":\"user\",\"content\":\"سؤال\"}]}","{\"q\":\"a\",\"n\":1,\"r\":\"b\"}","not json"],"max":10}"#)).unwrap();
        let p = r.get("previews").unwrap().as_arr().unwrap();
        assert_eq!(p[0].as_str().unwrap(), "سؤال");
        assert_eq!(p[1].as_str().unwrap(), "a · b · ");
        assert_eq!(p[2].as_str().unwrap(), "not json");
    }

    #[test]
    fn enter_alternates_roles() {
        let go = |before: &str| enter(&Value::obj(vec![("before", Value::str(before))])).unwrap();
        assert_eq!(go("User: hi").get("insert").unwrap().as_str().unwrap(), "Assistant: ");
        assert_eq!(go("User: hi\nAssistant: yo").get("insert").unwrap().as_str().unwrap(), "User: ");
        assert_eq!(go("System: be brief").get("insert").unwrap().as_str().unwrap(), "User: ");
        assert_eq!(go("Human: x").get("insert").unwrap().as_str().unwrap(), "Assistant: ");
        // multi-line message: label found on an earlier line
        assert_eq!(go("User: hi\nsecond line").get("insert").unwrap().as_str().unwrap(), "Assistant: ");
        assert_eq!(go("").get("insert").unwrap().as_str().unwrap(), "User: ");
        assert_eq!(go("User:").get("empty").unwrap().as_bool(), Some(true));
        assert_eq!(go("User: ").get("empty").unwrap().as_bool(), Some(true));
        assert_eq!(go("User: a").get("empty").unwrap().as_bool(), Some(false));
    }

    #[test]
    fn enter_fields_cycle() {
        let f = Value::Arr(vec![Value::str("question"), Value::str("answer")]);
        let go = |before: &str| enter(&Value::obj(vec![("before", Value::str(before)), ("mode", Value::str("fields")), ("fields", f.clone())])).unwrap();
        assert_eq!(go("question: q").get("insert").unwrap().as_str().unwrap(), "answer: ");
        assert_eq!(go("question: q\nanswer: a").get("insert").unwrap().as_str().unwrap(), "question: ");
    }

    #[test]
    fn release_parsing() {
        let j = r#"{"tag_name":"build-12","body":"notes","assets":[{"name":"x.txt","size":1},{"name":"MNX-12.apk","size":5000,"browser_download_url":"https://github.com/a/b/releases/download/build-12/MNX-12.apk"}]}"#;
        let mk = |cur: i64| release_info(&Value::obj(vec![("json", Value::str(j)), ("current", Value::int(cur))])).unwrap();
        assert_eq!(mk(11).get("available").unwrap().as_bool(), Some(true));
        assert_eq!(mk(12).get("available").unwrap().as_bool(), Some(false));
        assert_eq!(mk(1).get("size").unwrap().as_f64(), Some(5000.0));
        assert!(release_info(&Value::obj(vec![("json", Value::str("{\"tag_name\":\"nightly\"}"))])).is_err());
    }
}
