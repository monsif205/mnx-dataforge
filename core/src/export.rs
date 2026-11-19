//! Export profiles: convert stored records to the target training format.
//! Output is a list of JSON lines (one per record) plus per-record errors; the app streams
//! them to the destination file. Records are never modified in storage.

use crate::json::Value;

pub struct Profile {
    pub format: String, // "messages" | "sharegpt" | "alpaca" | "chatml" | "raw"
    pub role_map: Vec<(String, String)>,
    pub include_metadata: bool,
    pub system_in_alpaca: bool,
    pub thinking: String, // keep | reasoning_content | tags | drop
    pub attachments: String, // keep | inline | drop
}

impl Profile {
    pub fn from_value(v: &Value) -> Profile {
        let role_map = v
            .get("role_map")
            .and_then(|m| m.as_obj())
            .map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect())
            .unwrap_or_default();
        let format = v.get("format").and_then(|f| f.as_str()).unwrap_or("messages").to_string();
        let default_thinking = if format == "messages" || format == "raw" { "keep" } else { "tags" };
        let thinking = v.get("thinking").and_then(|f| f.as_str()).filter(|m| ["keep", "reasoning_content", "tags", "drop"].contains(m)).unwrap_or(default_thinking).to_string();
        let default_att = if format == "messages" || format == "raw" { "keep" } else { "inline" };
        let attachments = v.get("attachments").and_then(|f| f.as_str()).filter(|m| ["keep", "inline", "drop"].contains(m)).unwrap_or(default_att).to_string();
        Profile {
            attachments,
            thinking,
            format,
            role_map,
            include_metadata: v.get("include_metadata").and_then(|b| b.as_bool()).unwrap_or(false),
            system_in_alpaca: v.get("system_in_alpaca").and_then(|b| b.as_bool()).unwrap_or(true),
        }
    }
    /// Message text for formats that have no thinking field: <think>…</think> in front when mode is "tags".
    fn content_of(&self, m: &Value) -> String {
        let mut c = m.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string();
        if self.attachments == "inline" {
            c.push_str(&inline_attachments(m));
        }
        let c = c.as_str();
        match (self.thinking.as_str(), m.get("thinking").and_then(|t| t.as_str())) {
            ("tags", Some(t)) if !t.is_empty() => format!("<think>\n{}\n</think>\n{}", t, c),
            _ => c.to_string(),
        }
    }
    fn role(&self, r: &str) -> String {
        if let Some((_, to)) = self.role_map.iter().find(|(k, _)| k == r) {
            return to.clone();
        }
        if self.format == "sharegpt" {
            return match r {
                "user" => "human".into(),
                "assistant" => "gpt".into(),
                other => other.into(),
            };
        }
        r.to_string()
    }
}

/// Attachments as fenced blocks appended to the message text.
fn inline_attachments(m: &Value) -> String {
    let mut s = String::new();
    if let Some(list) = m.get("attachments").and_then(|a| a.as_arr()) {
        for a in list {
            let name = a.get("name").and_then(|x| x.as_str()).unwrap_or("file");
            let body = a.get("content").and_then(|x| x.as_str()).unwrap_or("");
            let fence = if body.contains("```") { "~~~~" } else { "```" };
            s.push_str(&format!("\n\n[file: {}]\n{}\n{}\n{}", name, fence, body.trim_end_matches('\n'), fence));
        }
    }
    s
}

/// Returns Ok(json value) for the line or Err(reason).
pub fn convert(rec: &Value, meta: Option<&Value>, p: &Profile) -> Result<Value, String> {
    let mut out = match p.format.as_str() {
        "raw" => rec.clone(),
        "messages" | "sharegpt" | "chatml" | "alpaca" => convert_conv(rec, p)?,
        other => return Err(format!("unknown export format '{}'", other)),
    };
    if p.include_metadata {
        if let (Some(m), Value::Obj(_)) = (meta, &out) {
            out.set("metadata", m.clone());
        }
    }
    Ok(out)
}

fn convert_conv(rec: &Value, p: &Profile) -> Result<Value, String> {
    let msgs = rec.get("messages").and_then(|m| m.as_arr()).ok_or("record has no 'messages' array")?;
    match p.format.as_str() {
        "messages" => {
            let mapped: Vec<Value> = msgs
                .iter()
                .map(|m| {
                    let mut m2 = m.clone();
                    if let Some(r) = m.get("role").and_then(|r| r.as_str()) {
                        m2.set("role", Value::str(p.role(r)));
                    }
                    if p.attachments != "keep" {
                        if let Value::Obj(items) = &mut m2 {
                            items.retain(|(k, _)| k != "attachments");
                        }
                    }
                    if let Some(t) = m.get("thinking").and_then(|t| t.as_str()).map(|t| t.to_string()) {
                        if let Value::Obj(items) = &mut m2 {
                            match p.thinking.as_str() {
                                "drop" => items.retain(|(k, _)| k != "thinking"),
                                "tags" => {
                                    items.retain(|(k, _)| k != "thinking");
                                    let c = p.content_of(m);
                                    for (k, v) in items.iter_mut() {
                                        if k == "content" {
                                            *v = Value::str(c.clone());
                                        }
                                    }
                                }
                                "reasoning_content" => {
                                    for (k, _) in items.iter_mut() {
                                        if k == "thinking" {
                                            *k = "reasoning_content".to_string();
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        let _ = t;
                    }
                    let tags_done = p.thinking == "tags" && m.get("thinking").and_then(|t| t.as_str()).map_or(false, |t| !t.is_empty());
                    if p.attachments == "inline" && !tags_done {
                        let extra = inline_attachments(m);
                        if !extra.is_empty() {
                            let c = format!("{}{}", m.get("content").and_then(|c| c.as_str()).unwrap_or(""), extra);
                            m2.set("content", Value::str(c));
                        }
                    }
                    m2
                })
                .collect();
            Ok(Value::obj(vec![("messages", Value::Arr(mapped))]))
        }
        "sharegpt" => {
            let mut conv = Vec::new();
            for m in msgs {
                let r = m.get("role").and_then(|r| r.as_str()).ok_or("message without role")?;
                let c = p.content_of(m);
                conv.push(Value::obj(vec![("from", Value::str(p.role(r))), ("value", Value::str(c))]));
            }
            Ok(Value::obj(vec![("conversations", Value::Arr(conv))]))
        }
        "chatml" => {
            let mut t = String::new();
            for m in msgs {
                let r = m.get("role").and_then(|r| r.as_str()).ok_or("message without role")?;
                let c = p.content_of(m);
                t.push_str("<|im_start|>");
                t.push_str(&p.role(r));
                t.push('\n');
                t.push_str(&c);
                t.push_str("<|im_end|>\n");
            }
            Ok(Value::obj(vec![("text", Value::str(t))]))
        }
        _ => {
            // alpaca: only single-turn (optional system + one user + one assistant).
            let get = |role: &str| -> Vec<&Value> { msgs.iter().filter(|m| m.get("role").and_then(|r| r.as_str()) == Some(role)).collect() };
            let (users, asst, sys) = (get("user"), get("assistant"), get("system"));
            if users.len() != 1 || asst.len() != 1 || msgs.iter().any(|m| m.get("role").and_then(|r| r.as_str()) == Some("tool")) {
                return Err("alpaca export supports only single-turn conversations".into());
            }
            let txt = |v: &Value| p.content_of(v);
            let mut pairs = vec![("instruction", Value::str(txt(users[0]))), ("input", Value::str(""))];
            pairs.push(("output", Value::str(txt(asst[0]))));
            if p.system_in_alpaca {
                if let Some(s) = sys.first() {
                    pairs.push(("system", Value::str(txt(s))));
                }
            }
            Ok(Value::obj(pairs))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    fn p(f: &str) -> Profile {
        Profile::from_value(&parse(&format!(r#"{{"format":"{}"}}"#, f)).unwrap())
    }
    fn conv() -> Value {
        parse(r#"{"messages":[{"role":"system","content":"s"},{"role":"user","content":"سؤال"},{"role":"assistant","content":"جواب"}]}"#).unwrap()
    }

    #[test]
    fn formats() {
        let sg = convert(&conv(), None, &p("sharegpt")).unwrap().to_json();
        assert!(sg.contains(r#""from":"human""#) && sg.contains(r#""from":"gpt""#) && sg.contains("جواب"));
        let al = convert(&conv(), None, &p("alpaca")).unwrap();
        assert_eq!(al.get("instruction").unwrap().as_str().unwrap(), "سؤال");
        assert!(convert(&conv(), None, &p("chatml")).unwrap().to_json().contains("<|im_start|>user"));
        assert!(convert(&conv(), None, &p("nope")).is_err());
    }

    #[test]
    fn alpaca_rejects_multi_turn() {
        let c = parse(r#"{"messages":[{"role":"user","content":"a"},{"role":"assistant","content":"b"},{"role":"user","content":"c"},{"role":"assistant","content":"d"}]}"#).unwrap();
        assert!(convert(&c, None, &p("alpaca")).is_err());
    }

    #[test]
    fn custom_role_map_and_metadata() {
        let prof = Profile::from_value(&parse(r#"{"format":"messages","role_map":{"assistant":"model"},"include_metadata":true}"#).unwrap());
        let meta = parse(r#"{"origin":"human"}"#).unwrap();
        let out = convert(&conv(), Some(&meta), &prof).unwrap();
        assert!(out.to_json().contains(r#""role":"model""#));
        assert!(out.get("metadata").is_some());
    }
}
