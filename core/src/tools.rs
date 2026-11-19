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

const CONV: [&str; 5] = ["System", "User", "Assistant", "Tool", "Thinking"];

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
            "reasoning" | "think" | "thought" => return Some("Thinking".into()),
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
            Some("Thinking") => "Assistant",
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

// ---------------------------------------------------------------- message editing

fn sget<'a>(o: &'a Value, k: &str) -> Option<&'a str> {
    o.get(k).and_then(|x| x.as_str())
}

fn set_key(obj: &mut Value, key: &str, val: Value) {
    if let Value::Obj(items) = obj {
        if let Some(p) = items.iter().position(|(k, _)| k == key) {
            items[p].1 = val;
        } else {
            items.push((key.to_string(), val));
        }
    } else {
        *obj = Value::Obj(vec![(key.to_string(), val)]);
    }
}

fn remove_key(obj: &mut Value, key: &str) {
    if let Value::Obj(items) = obj {
        items.retain(|(k, _)| k != key);
    }
}

/// Moves reasoning that other formats keep in "reasoning_content" / "reasoning" or inside <think> tags into the
/// "thinking" field of assistant messages. Returns true when anything changed.
pub fn normalize_thinking(msgs: &mut [Value]) -> bool {
    let mut changed = false;
    for m in msgs.iter_mut() {
        if sget(m, "role") != Some("assistant") {
            continue;
        }
        for k in ["reasoning_content", "reasoning"] {
            if let Some(t) = sget(m, k).map(|s| s.to_string()) {
                if m.get("thinking").is_none() && !t.trim().is_empty() {
                    set_key(m, "thinking", Value::str(t));
                }
                remove_key(m, k);
                changed = true;
            }
        }
        if let Some(c) = sget(m, "content").map(|s| s.to_string()) {
            let lead = c.trim_start();
            for (open, close) in [("<think>", "</think>"), ("<thinking>", "</thinking>")] {
                if lead.starts_with(open) {
                    if let Some(end) = lead.find(close) {
                        let inner = lead[open.len()..end].trim().to_string();
                        let rest = lead[end + close.len()..].trim_start().to_string();
                        if m.get("thinking").is_none() {
                            set_key(m, "thinking", Value::str(inner));
                        }
                        set_key(m, "content", Value::str(rest));
                        changed = true;
                        break;
                    }
                }
            }
        }
    }
    changed
}

fn messages_mut(root: &mut Value, create: bool) -> Result<&mut Vec<Value>, String> {
    let items = match root {
        Value::Obj(i) => i,
        _ => return Err("record is not a JSON object".into()),
    };
    let pos = match items.iter().position(|(k, _)| k == "messages") {
        Some(p) => p,
        None => {
            if !create {
                return Err("record has no messages array".into());
            }
            items.push(("messages".to_string(), Value::Arr(Vec::new())));
            items.len() - 1
        }
    };
    match &mut items[pos].1 {
        Value::Arr(a) => Ok(a),
        _ => Err("messages is not an array".into()),
    }
}

fn split_paragraphs(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for line in s.lines() {
        if line.trim().is_empty() {
            if !cur.is_empty() {
                out.push(cur.join("\n").trim().to_string());
                cur.clear();
            }
        } else {
            cur.push(line);
        }
    }
    if !cur.is_empty() {
        out.push(cur.join("\n").trim().to_string());
    }
    out
}

/// op "msg_edit": structural edits of a conversation record (all the message buttons of the editor).
/// actions: ensure, role, content, up, down, dup, split, merge, delete, add, turns.
pub fn msg_edit(v: &Value) -> Result<Value, String> {
    let doc = s_of(v, "doc")?;
    let action = s_of(v, "action")?;
    let idx = v.get("index").and_then(|x| x.as_f64()).map(|f| f.max(0.0) as usize);
    let mut root = parse(doc).map_err(|e| e.to_string())?;
    let create = matches!(action, "ensure" | "add" | "turns");
    {
        let msgs = messages_mut(&mut root, create)?;
        let need = |msgs: &Vec<Value>| -> Result<usize, String> {
            let i = idx.ok_or("missing 'index'")?;
            if i >= msgs.len() {
                Err(format!("index {} is out of range ({} messages)", i, msgs.len()))
            } else {
                Ok(i)
            }
        };
        match action {
            "ensure" => {}
            "role" => {
                let i = need(msgs)?;
                set_key(&mut msgs[i], "role", Value::str(s_of(v, "role")?));
            }
            "content" => {
                let i = need(msgs)?;
                set_key(&mut msgs[i], "content", Value::str(s_of(v, "content")?));
            }
            "thinking" => {
                let i = need(msgs)?;
                set_key(&mut msgs[i], "thinking", Value::str(s_of(v, "content")?));
            }
            "thinking_remove" => {
                let i = need(msgs)?;
                remove_key(&mut msgs[i], "thinking");
            }
            "up" => {
                let i = need(msgs)?;
                if i > 0 {
                    msgs.swap(i, i - 1);
                }
            }
            "down" => {
                let i = need(msgs)?;
                if i + 1 < msgs.len() {
                    msgs.swap(i, i + 1);
                }
            }
            "dup" => {
                let i = need(msgs)?;
                let c = msgs[i].clone();
                msgs.insert(i + 1, c);
            }
            "delete" => {
                let i = need(msgs)?;
                msgs.remove(i);
            }
            "split" => {
                let i = need(msgs)?;
                let parts = split_paragraphs(sget(&msgs[i], "content").unwrap_or(""));
                if parts.len() > 1 {
                    let base = msgs.remove(i);
                    for (k, part) in parts.into_iter().enumerate() {
                        let mut m = base.clone();
                        set_key(&mut m, "content", Value::str(part));
                        msgs.insert(i + k, m);
                    }
                }
            }
            "merge" => {
                let i = need(msgs)?;
                if i + 1 < msgs.len() {
                    let joined = format!("{}\n\n{}", sget(&msgs[i], "content").unwrap_or(""), sget(&msgs[i + 1], "content").unwrap_or(""));
                    set_key(&mut msgs[i], "content", Value::str(joined));
                    msgs.remove(i + 1);
                }
            }
            "add" => {
                let role = sget(v, "role").unwrap_or("user");
                let content = sget(v, "content").unwrap_or("");
                msgs.push(Value::obj(vec![("role", Value::str(role)), ("content", Value::str(content))]));
            }
            "turns" => {
                let n = v.get("count").and_then(|x| x.as_f64()).unwrap_or(1.0).clamp(1.0, 50.0) as usize;
                for _ in 0..n {
                    msgs.push(Value::obj(vec![("role", Value::str("user")), ("content", Value::str(""))]));
                    msgs.push(Value::obj(vec![("role", Value::str("assistant")), ("content", Value::str(""))]));
                }
            }
            other => return Err(format!("unknown action '{}'", other)),
        }
    }
    Ok(Value::obj(vec![("doc", Value::str(root.to_pretty(2)))]))
}

// ---------------------------------------------------------------- adapt (import formats -> messages)

fn adapt_one(o: &Value) -> Option<(Value, bool)> {
    if o.get("messages").is_some() {
        let mut c = o.clone();
        let mut changed = false;
        if let Value::Obj(items) = &mut c {
            for (k, v) in items.iter_mut() {
                if k == "messages" {
                    if let Value::Arr(ms) = v {
                        changed = normalize_thinking(ms);
                    }
                }
            }
        }
        return Some((c, changed));
    }
    if let Some(conv) = o.get("conversations") {
        let arr = conv.as_arr()?;
        let mut msgs = Vec::new();
        for m in arr {
            let role = match sget(m, "from")? {
                "human" | "user" => "user",
                "gpt" | "assistant" | "model" => "assistant",
                "system" => "system",
                "tool" | "function" => "tool",
                _ => return None,
            };
            msgs.push(Value::obj(vec![("role", Value::str(role)), ("content", Value::str(sget(m, "value").unwrap_or("")))]));
        }
        normalize_thinking(&mut msgs);
        return Some((Value::obj(vec![("messages", Value::Arr(msgs))]), true));
    }
    let user = if let Some(ins) = sget(o, "instruction") {
        match sget(o, "input") {
            Some(inp) if !inp.trim().is_empty() => format!("{}\n\n{}", ins, inp),
            _ => ins.to_string(),
        }
    } else {
        sget(o, "prompt")?.to_string()
    };
    let answer = sget(o, "output").or_else(|| sget(o, "completion")).or_else(|| sget(o, "response"))?;
    let mut msgs = Vec::new();
    if let Some(sys) = sget(o, "system") {
        if !sys.trim().is_empty() {
            msgs.push(Value::obj(vec![("role", Value::str("system")), ("content", Value::str(sys))]));
        }
    }
    msgs.push(Value::obj(vec![("role", Value::str("user")), ("content", Value::str(user))]));
    let mut am = vec![("role", Value::str("assistant")), ("content", Value::str(answer))];
    if let Some(t) = sget(o, "thinking").or_else(|| sget(o, "reasoning")).or_else(|| sget(o, "reasoning_content")) {
        if !t.trim().is_empty() {
            am.insert(1, ("thinking", Value::str(t)));
        }
    }
    msgs.push(Value::obj(am));
    normalize_thinking(&mut msgs);
    Some((Value::obj(vec![("messages", Value::Arr(msgs))]), true))
}

/// op "adapt": converts ShareGPT / Alpaca / prompt-completion records to {"messages": [...]}.
/// Result arrays are index-aligned with the input: `records[i]` is null when record i could not be converted.
pub fn adapt(v: &Value) -> Result<Value, String> {
    let recs = v.get("records").and_then(|r| r.as_arr()).ok_or("missing array field 'records'")?;
    let mut out = Vec::with_capacity(recs.len());
    let mut changed = Vec::with_capacity(recs.len());
    for r in recs {
        match adapt_one(r) {
            Some((x, c)) => {
                out.push(x);
                changed.push(Value::Bool(c));
            }
            None => {
                out.push(Value::Null);
                changed.push(Value::Bool(false));
            }
        }
    }
    Ok(Value::obj(vec![("records", Value::Arr(out)), ("changed", Value::Arr(changed))]))
}

// ---------------------------------------------------------------- template expansion

/// op "expand": every combination of the listed variable values is substituted into a JSON template.
pub fn expand(v: &Value) -> Result<Value, String> {
    let template = s_of(v, "template")?;
    let limit = v.get("limit").and_then(|x| x.as_f64()).unwrap_or(100.0).clamp(1.0, 5000.0) as usize;
    let vars: Vec<(String, Vec<String>)> = match v.get("vars") {
        Some(Value::Obj(items)) => items
            .iter()
            .map(|(k, val)| (k.clone(), val.as_arr().map(|a| a.iter().filter_map(|s| s.as_str().map(|t| t.to_string())).collect()).unwrap_or_default()))
            .collect(),
        _ => Vec::new(),
    };
    let mut problems: Vec<String> = Vec::new();
    let mut records: Vec<Value> = Vec::new();
    let mut combos: u64 = 1;
    for (_, vals) in &vars {
        combos = combos.saturating_mul(vals.len().max(1) as u64);
    }
    if combos > 1_000_000 {
        problems.push(format!("too many combinations ({}); reduce variable values", combos));
        return Ok(Value::obj(vec![("records", Value::Arr(records)), ("problems", Value::Arr(problems.into_iter().map(Value::str).collect()))]));
    }
    let mut seen = std::collections::HashSet::new();
    let mut idx = vec![0usize; vars.len()];
    let mut produced: u64 = 0;
    while produced < combos && records.len() < limit {
        let mut t = template.to_string();
        for (i, (name, vals)) in vars.iter().enumerate() {
            let raw = vals.get(idx[i]).map(|s| s.as_str()).unwrap_or("");
            let mut q = String::new();
            crate::json::escape_into(raw, &mut q);
            let inner = &q[1..q.len() - 1];
            t = t.replace(&format!("{{{{{}}}}}", name), inner);
        }
        produced += 1;
        match parse(&t) {
            Ok(o) => {
                if seen.insert(o.to_json()) {
                    records.push(o);
                }
            }
            Err(e) => {
                if problems.len() < 5 {
                    problems.push(format!("combination {} is not valid JSON: {}", produced, e));
                }
            }
        }
        let mut k = vars.len();
        loop {
            if k == 0 {
                k = usize::MAX;
                break;
            }
            k -= 1;
            idx[k] += 1;
            if idx[k] < vars[k].1.len() {
                break;
            }
            idx[k] = 0;
        }
        if k == usize::MAX {
            break;
        }
    }
    if records.len() >= limit && produced < combos {
        problems.push(format!("limited to {} records (of {} combinations)", limit, combos));
    }
    Ok(Value::obj(vec![("records", Value::Arr(records)), ("problems", Value::Arr(problems.into_iter().map(Value::str).collect()))]))
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
    fn msg_edit_actions() {
        let ed = |doc: &str, rest: &str| msg_edit(&parse(&format!("{{\"doc\":{},{}}}", Value::str(doc).to_json(), rest)).unwrap());
        let base = r#"{"messages":[{"role":"user","content":"a"},{"role":"assistant","content":"b\n\nc"}],"x":1}"#;
        let msgs = |r: Result<Value, String>| parse(r.unwrap().get("doc").unwrap().as_str().unwrap()).unwrap().get("messages").unwrap().as_arr().unwrap().clone();
        assert_eq!(msgs(ed(base, r#""action":"turns","count":2"#)).len(), 6);
        assert_eq!(msgs(ed(base, r#""action":"split","index":1"#)).len(), 3);
        assert_eq!(msgs(ed(base, r#""action":"dup","index":0"#)).len(), 3);
        assert_eq!(msgs(ed(base, r#""action":"delete","index":0"#)).len(), 1);
        let up = msgs(ed(base, r#""action":"up","index":1"#));
        assert_eq!(up[0].get("role").unwrap().as_str().unwrap(), "assistant");
        let merged = msgs(ed(base, r#""action":"merge","index":0"#));
        assert_eq!(merged[0].get("content").unwrap().as_str().unwrap(), "a\n\nb\n\nc");
        let role = msgs(ed(base, r#""action":"role","index":0,"role":"system""#));
        assert_eq!(role[0].get("role").unwrap().as_str().unwrap(), "system");
        assert!(ed(base, r#""action":"delete","index":9"#).is_err());
        assert!(ed("{\"q\":1}", r#""action":"delete","index":0"#).is_err());
        assert_eq!(msgs(ed("{\"q\":1}", r#""action":"add","role":"user""#)).len(), 1);
        // extra fields survive
        let r = ed(base, r#""action":"dup","index":0"#).unwrap();
        assert!(r.get("doc").unwrap().as_str().unwrap().contains("\"x\": 1"));
    }

    #[test]
    fn adapt_formats() {
        let r = adapt(&o(r#"{"records":[{"instruction":"i","input":"x","output":"o","system":"s"},{"conversations":[{"from":"human","value":"h"},{"from":"gpt","value":"g"}]},{"messages":[]},{"foo":1}]}"#)).unwrap();
        let recs = r.get("records").unwrap().as_arr().unwrap();
        assert_eq!(recs[0].get("messages").unwrap().as_arr().unwrap().len(), 3);
        assert_eq!(recs[0].get("messages").unwrap().as_arr().unwrap()[1].get("content").unwrap().as_str().unwrap(), "i\n\nx");
        assert_eq!(recs[1].get("messages").unwrap().as_arr().unwrap()[1].get("role").unwrap().as_str().unwrap(), "assistant");
        assert_eq!(recs[3], Value::Null);
        let ch = r.get("changed").unwrap().as_arr().unwrap();
        assert_eq!(ch[2].as_bool(), Some(false));
        assert_eq!(ch[0].as_bool(), Some(true));
    }

    #[test]
    fn expand_templates() {
        let r = expand(&o(r#"{"template":"{\"q\":\"{{a}}\",\"n\":\"{{b}}\"}","vars":{"a":["x","y\"z"],"b":["1","2","3"]},"limit":100}"#)).unwrap();
        assert_eq!(r.get("records").unwrap().as_arr().unwrap().len(), 6);
        let r = expand(&o(r#"{"template":"{\"q\":\"{{a}}\"}","vars":{"a":["x","x"]}}"#)).unwrap();
        assert_eq!(r.get("records").unwrap().as_arr().unwrap().len(), 1);
        let r = expand(&o(r#"{"template":"{\"q\":\"{{a}}\"}","vars":{"a":["1","2","3","4"]},"limit":2}"#)).unwrap();
        assert_eq!(r.get("records").unwrap().as_arr().unwrap().len(), 2);
        assert_eq!(r.get("problems").unwrap().as_arr().unwrap().len(), 1);
        let r = expand(&o(r#"{"template":"{\"q\":1}","vars":{}}"#)).unwrap();
        assert_eq!(r.get("records").unwrap().as_arr().unwrap().len(), 1);
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

#[cfg(test)]
mod thinking_tests {
    use super::*;
    use crate::json::parse;

    #[test]
    fn edit_add_and_remove_thinking() {
        let doc = r#"{"messages":[{"role":"user","content":"q"},{"role":"assistant","content":"a"}]}"#;
        let q = Value::obj(vec![("doc", Value::str(doc)), ("action", Value::str("thinking")), ("index", Value::int(1)), ("content", Value::str("step by step"))]);
        let out = msg_edit(&q).unwrap();
        let d = out.get("doc").unwrap().as_str().unwrap().to_string();
        assert!(d.contains("\"thinking\": \"step by step\""), "{}", d);
        let q2 = Value::obj(vec![("doc", Value::str(d)), ("action", Value::str("thinking_remove")), ("index", Value::int(1))]);
        assert!(!msg_edit(&q2).unwrap().get("doc").unwrap().as_str().unwrap().contains("thinking"));
    }

    #[test]
    fn import_forms_become_thinking() {
        let r = parse(r#"{"records":[
          {"messages":[{"role":"user","content":"q"},{"role":"assistant","content":"<think>because</think>\n\nanswer"}]},
          {"messages":[{"role":"user","content":"q"},{"role":"assistant","content":"a","reasoning_content":"why"}]},
          {"instruction":"i","output":"o","reasoning":"r"},
          {"messages":[{"role":"user","content":"q"},{"role":"assistant","content":"plain"}]}]}"#).unwrap();
        let out = adapt(&r).unwrap();
        let recs = out.get("records").unwrap().as_arr().unwrap();
        let ch = out.get("changed").unwrap().as_arr().unwrap();
        assert!(recs[0].to_json().contains("\"thinking\":\"because\"") && recs[0].to_json().contains("\"content\":\"answer\""));
        assert!(recs[1].to_json().contains("\"thinking\":\"why\"") && !recs[1].to_json().contains("reasoning_content"));
        assert!(recs[2].to_json().contains("\"thinking\":\"r\""));
        assert_eq!(ch[3].as_bool(), Some(false));
        assert_eq!(ch[0].as_bool(), Some(true));
    }

    #[test]
    fn quick_write_and_export_modes() {
        let b = crate::script::build(&parse(r#"{"text":"User: hi\nThinking: the user greets\nAssistant: hello"}"#).unwrap()).unwrap();
        let rec = b.get("records").unwrap().as_arr().unwrap()[0].clone();
        assert!(rec.to_json().contains("\"thinking\":\"the user greets\""), "{}", rec.to_json());
        let back = crate::script::render(&Value::obj(vec![("records", Value::Arr(vec![rec.clone()]))])).unwrap();
        assert!(back.get("text").unwrap().as_str().unwrap().contains("Thinking: the user greets"));
        let prof = |m: &str, f: &str| crate::export::Profile::from_value(&parse(&format!(r#"{{"format":"{}","thinking":"{}"}}"#, f, m)).unwrap());
        let tags = crate::export::convert(&rec, None, &prof("tags", "messages")).unwrap().to_json();
        assert!(tags.contains("<think>") && !tags.contains("\"thinking\""), "{}", tags);
        assert!(crate::export::convert(&rec, None, &prof("drop", "messages")).unwrap().to_json().find("thinking").is_none());
        assert!(crate::export::convert(&rec, None, &prof("reasoning_content", "messages")).unwrap().to_json().contains("reasoning_content"));
        assert!(crate::export::convert(&rec, None, &prof("keep", "chatml")).unwrap().to_json().find("think").is_none());
        assert!(crate::export::convert(&rec, None, &Profile2::default_for("chatml")).unwrap().to_json().contains("<think>"));
    }
    struct Profile2;
    impl Profile2 {
        fn default_for(f: &str) -> crate::export::Profile {
            crate::export::Profile::from_value(&parse(&format!(r#"{{"format":"{}"}}"#, f)).unwrap())
        }
    }
}
