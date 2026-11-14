//! Text helpers: hashing, Arabic-aware normalization, shingles, script detection.

use crate::json::Value;

pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn splitmix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// Lowercase, strip Arabic diacritics/tatweel/bidi marks, fold alef/yaa variants,
/// replace punctuation with single spaces.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for c in s.chars() {
        let m = match c {
            '\u{064B}'..='\u{065F}' | '\u{0670}' | '\u{0640}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' => continue,
            'أ' | 'إ' | 'آ' | 'ٱ' => 'ا',
            'ى' => 'ي',
            _ => c,
        };
        if m.is_alphanumeric() {
            for lc in m.to_lowercase() {
                out.push(lc);
            }
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Sorted, de-duplicated set of 3-word shingles (or 4-char shingles for very short text).
pub fn shingles(norm: &str) -> Vec<u64> {
    let words: Vec<&str> = norm.split(' ').filter(|w| !w.is_empty()).collect();
    let mut set: Vec<u64> = Vec::new();
    if words.len() >= 3 {
        for w in words.windows(3) {
            let joined = format!("{} {} {}", w[0], w[1], w[2]);
            set.push(fnv1a(joined.as_bytes()));
        }
    } else {
        let chars: Vec<char> = norm.chars().collect();
        if chars.len() >= 4 {
            for w in chars.windows(4) {
                let s: String = w.iter().collect();
                set.push(fnv1a(s.as_bytes()));
            }
        } else if !norm.is_empty() {
            set.push(fnv1a(norm.as_bytes()));
        }
    }
    set.sort_unstable();
    set.dedup();
    set
}

pub fn jaccard(a: &[u64], b: &[u64]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let (mut i, mut j, mut inter) = (0usize, 0usize, 0usize);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            inter += 1;
            i += 1;
            j += 1;
        } else if a[i] < b[j] {
            i += 1;
        } else {
            j += 1;
        }
    }
    inter as f64 / (a.len() + b.len() - inter) as f64
}

/// Script-based label (not real language identification).
pub fn script_of(s: &str) -> &'static str {
    let (mut ar, mut la, mut other) = (0usize, 0usize, 0usize);
    for c in s.chars() {
        if !c.is_alphabetic() {
            continue;
        }
        let u = c as u32;
        if (0x0600..=0x06FF).contains(&u) || (0x0750..=0x077F).contains(&u) || (0xFB50..=0xFDFF).contains(&u) || (0xFE70..=0xFEFF).contains(&u) {
            ar += 1;
        } else if c.is_ascii_alphabetic() || (0x00C0..=0x024F).contains(&u) {
            la += 1;
        } else {
            other += 1;
        }
    }
    let total = ar + la + other;
    if total == 0 {
        "none"
    } else if ar * 10 >= total * 8 {
        "arabic"
    } else if la * 10 >= total * 8 {
        "latin"
    } else if other * 2 > total {
        "other"
    } else {
        "mixed"
    }
}

fn collect_strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Str(s) => out.push(s.clone()),
        Value::Arr(a) => a.iter().for_each(|x| collect_strings(x, out)),
        Value::Obj(o) => o.iter().for_each(|(_, x)| collect_strings(x, out)),
        _ => {}
    }
}

/// Canonical text of a record used for duplicate / length / language analysis.
/// Conversations use only message contents; other records use all string values.
pub fn record_text(v: &Value) -> String {
    if let Some(msgs) = v.get("messages").and_then(|m| m.as_arr()) {
        let parts: Vec<&str> = msgs.iter().filter_map(|m| m.get("content").and_then(|c| c.as_str())).collect();
        return parts.join("\n");
    }
    let mut parts = Vec::new();
    collect_strings(v, &mut parts);
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arabic_folding() {
        assert_eq!(normalize("أَحْمَد"), normalize("احمد"));
        assert_eq!(normalize("إلى"), normalize("الي"));
        assert_eq!(normalize("Hello,  WORLD!"), "hello world");
    }

    #[test]
    fn jaccard_basics() {
        let a = shingles(&normalize("the quick brown fox jumps over the lazy dog"));
        let b = shingles(&normalize("the quick brown fox jumps over the lazy cat"));
        let j = jaccard(&a, &b);
        assert!(j > 0.4 && j < 1.0);
        assert_eq!(jaccard(&a, &a), 1.0);
    }

    #[test]
    fn scripts() {
        assert_eq!(script_of("مرحبا بالعالم"), "arabic");
        assert_eq!(script_of("hello world"), "latin");
        assert_eq!(script_of("12345"), "none");
        assert_eq!(script_of("hello مرحبا"), "mixed");
    }
}
