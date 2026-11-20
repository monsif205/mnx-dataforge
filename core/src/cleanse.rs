//! Dataset cleanup: PII masking, boilerplate and repeated-line removal, and record filters (length, repetition, quality, language).
//! Works on the text fields of a record (messages[].content/thinking, text, prompt, completion, instruction, input, output).
//! Every decision is reported with its reason; nothing is applied to storage here.

use crate::json::Value;
use crate::nlp;

pub struct Opts {
    pub pii: Vec<String>, // email | phone | ip | url
    pub boilerplate: bool,
    pub repeat_lines: bool,
    pub min_chars: usize,
    pub min_quality: f64,  // 0 = off
    pub max_repeat: f64,   // share of repeated words above which a record is dropped; 0 = off
    pub lang: String,      // "" = any
}

impl Opts {
    pub fn from_value(v: &Value) -> Opts {
        let pii = v.get("pii").and_then(|a| a.as_arr()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
        let b = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        let f = |k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
        Opts { pii, boilerplate: b("boilerplate"), repeat_lines: b("repeat_lines"), min_chars: f("min_chars").max(0.0) as usize, min_quality: f("min_quality"), max_repeat: f("max_repeat"), lang: v.get("lang").and_then(|x| x.as_str()).unwrap_or("").to_string() }
    }
}

#[derive(Default, Debug)]
pub struct Counts {
    pub email: usize,
    pub phone: usize,
    pub ip: usize,
    pub url: usize,
    pub boilerplate_lines: usize,
    pub repeated_lines: usize,
}

fn is_local(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-')
}

fn mask_email(s: &str, n: &mut usize) -> String {
    let ch: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < ch.len() {
        if ch[i] == '@' && i > 0 && is_local(ch[i - 1]) {
            let mut j = i + 1;
            while j < ch.len() && (ch[j].is_alphanumeric() || ch[j] == '-' || ch[j] == '.') {
                j += 1;
            }
            while j > i + 1 && ch[j - 1] == '.' {
                j -= 1;
            }
            let dom: String = ch[i + 1..j].iter().collect();
            if dom.contains('.') && !dom.starts_with('.') {
                let mut k = out.chars().count();
                let outc: Vec<char> = out.chars().collect();
                while k > 0 && is_local(outc[k - 1]) {
                    k -= 1;
                }
                out = outc[..k].iter().collect();
                out.push_str("[EMAIL]");
                *n += 1;
                i = j;
                continue;
            }
        }
        out.push(ch[i]);
        i += 1;
    }
    out
}

fn mask_urls(s: &str, n: &mut usize) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        let lower = rest.to_ascii_lowercase();
        let pos = ["http://", "https://", "www."].iter().filter_map(|p| lower.find(p)).min();
        match pos {
            Some(p) => {
                out.push_str(&rest[..p]);
                let tail = &rest[p..];
                let end = tail.find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | ')' | ']')).unwrap_or(tail.len());
                let mut e = end;
                while e > 0 && matches!(tail.as_bytes()[e - 1], b'.' | b',' | b';' | b':' | b'!' | b'?') {
                    e -= 1;
                }
                out.push_str("[URL]");
                *n += 1;
                rest = &tail[e..];
            }
            None => {
                out.push_str(rest);
                return out;
            }
        }
    }
}

fn mask_ip(s: &str, n: &mut usize) -> String {
    let ch: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < ch.len() {
        if ch[i].is_ascii_digit() && (i == 0 || !(ch[i - 1].is_alphanumeric() || ch[i - 1] == '.')) {
            let mut j = i;
            let mut parts = 0;
            let mut ok = true;
            loop {
                let st = j;
                while j < ch.len() && ch[j].is_ascii_digit() {
                    j += 1;
                }
                let len = j - st;
                if len == 0 || len > 3 || ch[st..j].iter().collect::<String>().parse::<u32>().unwrap_or(999) > 255 {
                    ok = false;
                    break;
                }
                parts += 1;
                if parts == 4 {
                    break;
                }
                if j < ch.len() && ch[j] == '.' {
                    j += 1;
                } else {
                    ok = false;
                    break;
                }
            }
            if ok && parts == 4 && (j >= ch.len() || !(ch[j].is_alphanumeric() || ch[j] == '.' && ch.get(j + 1).map_or(false, |c| c.is_ascii_digit()))) {
                out.push_str("[IP]");
                *n += 1;
                i = j;
                continue;
            }
        }
        out.push(ch[i]);
        i += 1;
    }
    out
}

fn mask_phone(s: &str, n: &mut usize) -> String {
    let ch: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < ch.len() {
        let start_ok = ch[i] == '+' || ch[i].is_ascii_digit() || (ch[i] == '(' && ch.get(i + 1).map_or(false, |c| c.is_ascii_digit()));
        if start_ok && (i == 0 || !(ch[i - 1].is_alphanumeric() || ch[i - 1] == '.')) {
            let mut j = i;
            let mut digits = 0;
            let mut last_digit = i;
            while j < ch.len() {
                let c = ch[j];
                if c.is_ascii_digit() {
                    digits += 1;
                    last_digit = j;
                } else if !(matches!(c, ' ' | '-' | '(' | ')' | '+') || (c == '.' && false)) {
                    break;
                } else if c == ' ' && ch.get(j + 1).map_or(true, |d| !d.is_ascii_digit() && *d != '(' && *d != '-') {
                    break;
                }
                j += 1;
            }
            let grouped = ch[i..=last_digit].iter().any(|c| matches!(c, ' ' | '-' | '(' | '+')) || digits >= 10;
            if (9..=15).contains(&digits) && grouped {
                out.push_str("[PHONE]");
                *n += 1;
                i = last_digit + 1;
                continue;
            }
        }
        out.push(ch[i]);
        i += 1;
    }
    out
}

const BOILER: [&str; 18] = [
    "all rights reserved", "accept all cookies", "we use cookies", "cookie policy", "privacy policy", "terms of use", "terms and conditions", "skip to content", "skip to main content",
    "share on facebook", "share on twitter", "follow us on", "subscribe to our newsletter", "sign up for our newsletter", "click here to", "back to top", "tous droits réservés", "جميع الحقوق محفوظة",
];

fn is_boiler(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() || t.chars().count() > 90 {
        return false;
    }
    let l = t.to_lowercase();
    if t.starts_with('©') || BOILER.iter().any(|b| l.contains(b)) {
        return true;
    }
    // "Page 3", "3 / 12", lone page numbers
    let digits_only = t.chars().all(|c| c.is_ascii_digit());
    digits_only && t.len() <= 3 && false
}

pub fn clean_text(t: &str, o: &Opts, c: &mut Counts) -> String {
    let mut s = t.to_string();
    if o.boilerplate || o.repeat_lines {
        let lines: Vec<&str> = s.split('\n').collect();
        let mut freq: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for l in &lines {
            let k = l.trim();
            if !k.is_empty() && k.chars().count() <= 120 {
                *freq.entry(k).or_insert(0) += 1;
            }
        }
        let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        let mut kept: Vec<&str> = Vec::new();
        let mut prev_blank = false;
        for l in lines {
            let k = l.trim();
            if o.boilerplate && is_boiler(l) {
                c.boilerplate_lines += 1;
                continue;
            }
            if o.repeat_lines && !k.is_empty() && k.chars().count() <= 120 {
                let n = seen.entry(k).or_insert(0);
                *n += 1;
                if *n > 1 && freq.get(k).copied().unwrap_or(0) >= 3 {
                    c.repeated_lines += 1;
                    continue;
                }
            }
            if k.is_empty() {
                if prev_blank {
                    continue;
                }
                prev_blank = true;
            } else {
                prev_blank = false;
            }
            kept.push(l);
        }
        s = kept.join("\n").trim().to_string();
    }
    for p in &o.pii {
        s = match p.as_str() {
            "url" => mask_urls(&s, &mut c.url),
            "email" => mask_email(&s, &mut c.email),
            "ip" => mask_ip(&s, &mut c.ip),
            "phone" => mask_phone(&s, &mut c.phone),
            _ => s,
        };
    }
    s
}

const KEYS: [&str; 7] = ["text", "prompt", "completion", "instruction", "input", "output", "thinking"];

fn walk(v: &mut Value, o: &Opts, c: &mut Counts, in_message: bool) {
    match v {
        Value::Obj(items) => {
            for (k, x) in items.iter_mut() {
                match x {
                    Value::Str(s) if KEYS.contains(&k.as_str()) || (in_message && k == "content") => {
                        let n = clean_text(s, o, c);
                        *s = n;
                    }
                    Value::Arr(_) if k == "messages" => walk(x, o, c, true),
                    Value::Arr(_) if k == "attachments" => walk(x, o, c, true),
                    Value::Obj(_) => walk(x, o, c, in_message),
                    _ => {}
                }
            }
        }
        Value::Arr(a) => {
            for x in a.iter_mut() {
                walk(x, o, c, in_message);
            }
        }
        _ => {}
    }
}

fn repeat_share(text: &str) -> f64 {
    let ws = nlp::words(text);
    if ws.len() < 20 {
        return 0.0;
    }
    let mut set = std::collections::HashSet::new();
    let uniq = ws.iter().filter(|w| set.insert(w.to_lowercase())).count();
    1.0 - uniq as f64 / ws.len() as f64
}

/// Returns (cleaned record, reasons to drop it (empty = keep), whether the text changed).
pub fn process(rec: &Value, o: &Opts, c: &mut Counts) -> (Value, Vec<String>, bool) {
    let mut out = rec.clone();
    walk(&mut out, o, c, false);
    let changed = out.to_json() != rec.to_json();
    let text = crate::text::record_text(&out);
    let mut why = Vec::new();
    if o.min_chars > 0 && text.trim().chars().count() < o.min_chars {
        why.push(format!("shorter than {} characters", o.min_chars));
    }
    if o.min_quality > 0.0 {
        let (q, r) = nlp::quality(&text);
        if q < o.min_quality {
            why.push(format!("quality {:.0} below {:.0} ({})", q, o.min_quality, r.join(", ")));
        }
    }
    if o.max_repeat > 0.0 && repeat_share(&text) > o.max_repeat {
        why.push("too many repeated words".to_string());
    }
    if !o.lang.is_empty() {
        let (l, conf) = nlp::detect_lang(&text);
        if conf >= 0.5 && l != o.lang {
            why.push(format!("language {} (wanted {})", l, o.lang));
        }
    }
    (out, why, changed)
}

/// op "cleanse": {records:[...], options:{...}} -> {results:[{action, record?, reasons}], counts}
pub fn op(v: &Value) -> Result<Value, String> {
    let recs = v.get("records").and_then(|r| r.as_arr()).ok_or("missing 'records'")?;
    let o = Opts::from_value(v.get("options").unwrap_or(&Value::Null));
    let mut c = Counts::default();
    let mut results = Vec::with_capacity(recs.len());
    for r in recs {
        let (out, why, changed) = process(r, &o, &mut c);
        results.push(if !why.is_empty() {
            Value::obj(vec![("action", Value::str("drop")), ("reasons", Value::Arr(why.into_iter().map(Value::str).collect()))])
        } else if changed {
            Value::obj(vec![("action", Value::str("change")), ("record", out)])
        } else {
            Value::obj(vec![("action", Value::str("keep"))])
        });
    }
    let n = |x: usize| Value::int(x as i64);
    Ok(Value::obj(vec![
        ("results", Value::Arr(results)),
        ("counts", Value::obj(vec![("email", n(c.email)), ("phone", n(c.phone)), ("ip", n(c.ip)), ("url", n(c.url)), ("boilerplate_lines", n(c.boilerplate_lines)), ("repeated_lines", n(c.repeated_lines))])),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn all() -> Opts {
        Opts { pii: vec!["url".into(), "email".into(), "ip".into(), "phone".into()], boilerplate: true, repeat_lines: true, min_chars: 0, min_quality: 0.0, max_repeat: 0.0, lang: String::new() }
    }
    #[test]
    fn pii_masks() {
        let mut c = Counts::default();
        let t = clean_text("Mail me: ali.b+x@mail.example.com or see https://example.com/a?b=1. Server 192.168.1.20, call +212 661-234567 now. Version 1.2.3.4.5 stays.", &all(), &mut c);
        assert_eq!(t, "Mail me: [EMAIL] or see [URL]. Server [IP], call [PHONE] now. Version 1.2.3.4.5 stays.", "{}", t);
        assert_eq!((c.email, c.url, c.ip, c.phone), (1, 1, 1, 1));
    }
    #[test]
    fn boilerplate_and_repeats() {
        let mut c = Counts::default();
        let t = clean_text("Real content here.\nAll rights reserved\nMenu\nMenu\nMenu\nMenu\nMore content.", &all(), &mut c);
        assert_eq!(t, "Real content here.\nMenu\nMore content.", "{}", t);
        assert_eq!(c.boilerplate_lines, 1);
        assert_eq!(c.repeated_lines, 3);
    }
    #[test]
    fn record_filters_and_messages() {
        let rec = crate::json::parse(r#"{"messages":[{"role":"user","content":"hi"},{"role":"assistant","content":"write to a@b.co"}]}"#).unwrap();
        let o = Opts { min_chars: 100, ..all() };
        let (out, why, changed) = process(&rec, &o, &mut Counts::default());
        assert!(changed && out.to_json().contains("[EMAIL]") && why.len() == 1);
        let r = op(&crate::json::parse(r#"{"records":[{"text":"ok [x]"}],"options":{"pii":["email"]}}"#).unwrap()).unwrap();
        assert!(r.to_json().contains("\"keep\""));
    }
}
