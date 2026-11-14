//! Strict JSON parser and writer. Preserves object key order and raw number text,
//! keeps Unicode (including Arabic) as-is when writing.

use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

#[derive(Debug)]
pub struct JsonError {
    pub pos: usize,
    pub msg: String,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.msg, self.pos)
    }
}

const MAX_DEPTH: usize = 128;

pub fn parse(s: &str) -> Result<Value, JsonError> {
    let mut p = Parser { b: s.as_bytes(), s, i: 0 };
    // Tolerate a UTF-8 BOM.
    if p.b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        p.i = 3;
    }
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i != p.b.len() {
        return Err(p.err("trailing characters"));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    s: &'a str,
    i: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, m: &str) -> JsonError {
        JsonError { pos: self.i, msg: m.to_string() }
    }

    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, JsonError> {
        if depth > MAX_DEPTH {
            return Err(self.err("nesting too deep"));
        }
        match self.b.get(self.i).copied() {
            None => Err(self.err("unexpected end of input")),
            Some(b'n') => self.lit("null", Value::Null),
            Some(b't') => self.lit("true", Value::Bool(true)),
            Some(b'f') => self.lit("false", Value::Bool(false)),
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b'[') => self.array(depth),
            Some(b'{') => self.object(depth),
            Some(b'-') | Some(b'0'..=b'9') => self.number(),
            _ => Err(self.err("unexpected character")),
        }
    }

    fn lit(&mut self, word: &str, v: Value) -> Result<Value, JsonError> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(self.err("invalid literal"))
        }
    }

    fn digits(&mut self) -> usize {
        let st = self.i;
        while self.i < self.b.len() && self.b[self.i].is_ascii_digit() {
            self.i += 1;
        }
        self.i - st
    }

    fn number(&mut self) -> Result<Value, JsonError> {
        let start = self.i;
        if self.b[self.i] == b'-' {
            self.i += 1;
        }
        match self.b.get(self.i) {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.err("invalid number")),
        }
        if self.b.get(self.i) == Some(&b'.') {
            self.i += 1;
            if self.digits() == 0 {
                return Err(self.err("invalid number fraction"));
            }
        }
        if matches!(self.b.get(self.i), Some(b'e') | Some(b'E')) {
            self.i += 1;
            if matches!(self.b.get(self.i), Some(b'+') | Some(b'-')) {
                self.i += 1;
            }
            if self.digits() == 0 {
                return Err(self.err("invalid number exponent"));
            }
        }
        Ok(Value::Num(self.s[start..self.i].to_string()))
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        if self.i + 4 > self.b.len() {
            return Err(self.err("short unicode escape"));
        }
        let mut v = 0u32;
        for k in 0..4 {
            let c = self.b[self.i + k];
            let d = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => return Err(self.err("bad hex digit")),
            };
            v = v * 16 + d as u32;
        }
        self.i += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.i += 1; // opening quote
        let mut out = String::new();
        let mut run = self.i;
        loop {
            match self.b.get(self.i).copied() {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => {
                    out.push_str(&self.s[run..self.i]);
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    out.push_str(&self.s[run..self.i]);
                    self.i += 1;
                    let e = self.b.get(self.i).copied().ok_or_else(|| self.err("bad escape"))?;
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{08}'),
                        b'f' => out.push('\u{0C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let cp = if (0xD800..0xDC00).contains(&hi) {
                                if self.b.get(self.i) == Some(&b'\\') && self.b.get(self.i + 1) == Some(&b'u') {
                                    self.i += 2;
                                    let lo = self.hex4()?;
                                    if !(0xDC00..0xE000).contains(&lo) {
                                        return Err(self.err("invalid low surrogate"));
                                    }
                                    0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                                } else {
                                    return Err(self.err("lone high surrogate"));
                                }
                            } else if (0xDC00..0xE000).contains(&hi) {
                                return Err(self.err("lone low surrogate"));
                            } else {
                                hi
                            };
                            out.push(char::from_u32(cp).ok_or_else(|| self.err("bad code point"))?);
                        }
                        _ => return Err(self.err("invalid escape")),
                    }
                    run = self.i;
                }
                Some(c) if c < 0x20 => return Err(self.err("control character in string")),
                Some(_) => self.i += 1,
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, JsonError> {
        self.i += 1;
        let mut items = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b']') {
            self.i += 1;
            return Ok(Value::Arr(items));
        }
        loop {
            self.ws();
            items.push(self.value(depth + 1)?);
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Value::Arr(items));
                }
                _ => return Err(self.err("expected ',' or ']'")),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, JsonError> {
        self.i += 1;
        let mut items: Vec<(String, Value)> = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b'}') {
            self.i += 1;
            return Ok(Value::Obj(items));
        }
        loop {
            self.ws();
            if self.b.get(self.i) != Some(&b'"') {
                return Err(self.err("expected string key"));
            }
            let k = self.string()?;
            self.ws();
            if self.b.get(self.i) != Some(&b':') {
                return Err(self.err("expected ':'"));
            }
            self.i += 1;
            self.ws();
            let v = self.value(depth + 1)?;
            // Duplicate keys: last one wins, position of first is kept.
            if let Some(slot) = items.iter_mut().find(|(ek, _)| *ek == k) {
                slot.1 = v;
            } else {
                items.push((k, v));
            }
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Value::Obj(items));
                }
                _ => return Err(self.err("expected ',' or '}'")),
            }
        }
    }
}

pub fn escape_into(s: &str, o: &mut String) {
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
}

impl Value {
    pub fn str<S: Into<String>>(s: S) -> Value {
        Value::Str(s.into())
    }
    pub fn int(n: i64) -> Value {
        Value::Num(n.to_string())
    }
    pub fn float(f: f64) -> Value {
        if f.is_finite() {
            let mut t = format!("{}", f);
            if !t.contains('.') && !t.contains('e') {
                t.push_str(".0");
            }
            Value::Num(t)
        } else {
            Value::Null
        }
    }
    pub fn obj(pairs: Vec<(&str, Value)>) -> Value {
        Value::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn get(&self, k: &str) -> Option<&Value> {
        match self {
            Value::Obj(items) => items.iter().find(|(ik, _)| ik == k).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Dotted-path lookup, e.g. "meta.source".
    pub fn get_path(&self, path: &str) -> Option<&Value> {
        let mut cur = self;
        for part in path.split('.') {
            cur = cur.get(part)?;
        }
        Some(cur)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Arr(a) => Some(a),
            _ => None,
        }
    }
    pub fn as_obj(&self) -> Option<&Vec<(String, Value)>> {
        match self {
            Value::Obj(o) => Some(o),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Num(n) => n.parse().ok(),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn set(&mut self, k: &str, v: Value) {
        if let Value::Obj(items) = self {
            if let Some(slot) = items.iter_mut().find(|(ik, _)| ik == k) {
                slot.1 = v;
            } else {
                items.push((k.to_string(), v));
            }
        }
    }

    pub fn to_json(&self) -> String {
        let mut o = String::new();
        self.write(&mut o);
        o
    }

    pub fn write(&self, o: &mut String) {
        match self {
            Value::Null => o.push_str("null"),
            Value::Bool(true) => o.push_str("true"),
            Value::Bool(false) => o.push_str("false"),
            Value::Num(n) => o.push_str(n),
            Value::Str(s) => escape_into(s, o),
            Value::Arr(a) => {
                o.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        o.push(',');
                    }
                    v.write(o);
                }
                o.push(']');
            }
            Value::Obj(items) => {
                o.push('{');
                for (i, (k, v)) in items.iter().enumerate() {
                    if i > 0 {
                        o.push(',');
                    }
                    escape_into(k, o);
                    o.push(':');
                    v.write(o);
                }
                o.push('}');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_arabic_and_escapes() {
        let src = r#"{"q":"ما هي عاصمة المغرب؟","a":"الرباط","n":-1.5e3,"t":[true,null,"x\n\"y\""]}"#;
        let v = parse(src).unwrap();
        assert_eq!(v.get("q").unwrap().as_str().unwrap(), "ما هي عاصمة المغرب؟");
        let again = parse(&v.to_json()).unwrap();
        assert_eq!(v, again);
        assert!(v.to_json().contains("الرباط"), "Arabic must not be escaped");
    }

    #[test]
    fn surrogate_pairs() {
        let v = parse(r#""😀""#).unwrap();
        assert_eq!(v.as_str().unwrap(), "😀");
        assert!(parse(r#""\ud83d""#).is_err());
        assert!(parse(r#""\ude00""#).is_err());
    }

    #[test]
    fn rejects_invalid() {
        for bad in ["{", "[1,]", "{\"a\":}", "01", "1.", "\"a\nb\"", "{'a':1}", "[1] x", "nul"] {
            assert!(parse(bad).is_err(), "should reject {:?}", bad);
        }
    }

    #[test]
    fn depth_limit() {
        let deep = "[".repeat(500) + &"]".repeat(500);
        assert!(parse(&deep).is_err());
    }

    #[test]
    fn key_order_preserved() {
        let v = parse(r#"{"z":1,"a":2,"m":3}"#).unwrap();
        assert_eq!(v.to_json(), r#"{"z":1,"a":2,"m":3}"#);
    }
}
