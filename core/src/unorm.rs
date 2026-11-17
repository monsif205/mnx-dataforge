//! Unicode text cleaning: compatibility folding (NFKC-style, for the ranges that matter for web and PDF text:
//! fullwidth forms, ligatures, Arabic presentation forms, special spaces), canonical composition of the
//! common combining marks (NFC-style), invisible-character removal and whitespace normalization.
//! Tables come from unorm_data.rs (generated). Limitation: canonical reordering of stacked marks is not done.

use crate::unorm_data::{COMPAT, COMPOSE};

#[derive(Clone, Debug)]
pub struct CleanOpts {
    pub compat: bool,
    pub compose: bool,
    pub strip_invisible: bool,
    pub strip_tatweel: bool,
    pub strip_arabic_marks: bool, // tashkeel (harakat)
    pub fold_arabic_letters: bool, // alef variants, alef maqsura, teh marbuta -> unified forms
    pub collapse_ws: bool,         // collapse runs of spaces/tabs; trim line ends; keep newlines
    pub max_blank_lines: usize,    // runs of blank lines are cut to this many (paragraph break = 1)
}

impl Default for CleanOpts {
    fn default() -> Self {
        CleanOpts { compat: true, compose: true, strip_invisible: true, strip_tatweel: true, strip_arabic_marks: false, fold_arabic_letters: false, collapse_ws: true, max_blank_lines: 1 }
    }
}

pub fn compat_of(c: char) -> Option<&'static str> {
    COMPAT.binary_search_by_key(&(c as u32), |e| e.0).ok().map(|i| COMPAT[i].1)
}

fn compose_pair(a: char, b: char) -> Option<char> {
    COMPOSE.binary_search_by(|e| (e.0, e.1).cmp(&(a as u32, b as u32))).ok().and_then(|i| char::from_u32(COMPOSE[i].2))
}

fn is_combining(c: char) -> bool {
    let u = c as u32;
    (0x300..=0x36F).contains(&u) || (0x653..=0x655).contains(&u) || u == 0x3099 || u == 0x309A
}

fn is_invisible(c: char) -> bool {
    matches!(c as u32, 0xAD | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x2069 | 0xFEFF | 0x061C | 0x180E)
        || (c.is_control() && c != '\n' && c != '\t' && c != '\r')
}

fn is_arabic_mark(c: char) -> bool {
    matches!(c as u32, 0x64B..=0x65F | 0x670 | 0x6D6..=0x6DC | 0x6DF..=0x6E4 | 0x6E7 | 0x6E8 | 0x6EA..=0x6ED)
}

fn fold_ar(c: char) -> char {
    match c {
        'أ' | 'إ' | 'آ' | 'ٱ' => 'ا',
        'ى' => 'ي',
        'ة' => 'ه',
        'ک' => 'ك',
        'ی' => 'ي',
        _ => c,
    }
}

pub fn clean(s: &str, o: &CleanOpts) -> String {
    // pass 1: per-character mapping
    let mut tmp = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\r' {
            tmp.push('\n');
            continue;
        }
        if o.strip_invisible && is_invisible(c) {
            continue;
        }
        if o.strip_tatweel && c == '\u{640}' {
            continue;
        }
        if o.strip_arabic_marks && is_arabic_mark(c) {
            continue;
        }
        let c2 = if o.fold_arabic_letters { fold_ar(c) } else { c };
        if o.compat && (c2 as u32) >= 0xA0 {
            if let Some(r) = compat_of(c2) {
                tmp.push_str(r);
                continue;
            }
        }
        if c2.is_whitespace() && c2 != '\n' && c2 != '\t' {
            tmp.push(' ');
        } else {
            tmp.push(c2);
        }
    }
    // pass 2: composition
    let composed: String = if o.compose && tmp.chars().any(is_combining) {
        let mut out: Vec<char> = Vec::with_capacity(tmp.len());
        let mut starter: Option<usize> = None;
        for c in tmp.chars() {
            if is_combining(c) {
                if let Some(si) = starter {
                    if si + 1 == out.len() || out[si + 1..].iter().all(|x| is_combining(*x)) && si + 1 == out.len() {
                        if let Some(n) = compose_pair(out[si], c) {
                            out[si] = n;
                            continue;
                        }
                    }
                }
                out.push(c);
            } else {
                out.push(c);
                starter = Some(out.len() - 1);
            }
        }
        out.into_iter().collect()
    } else {
        tmp
    };
    if !o.collapse_ws {
        return composed;
    }
    collapse_whitespace(&composed, o.max_blank_lines)
}

/// Collapses runs of spaces/tabs inside lines, trims line ends and cuts runs of blank lines.
pub fn collapse_whitespace(s: &str, max_blank: usize) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blanks = 0usize;
    let mut first = true;
    for line in s.split('\n') {
        let mut l = String::with_capacity(line.len());
        let mut prev_space = false;
        for c in line.chars() {
            if c == ' ' || c == '\t' {
                if !prev_space {
                    l.push(' ');
                }
                prev_space = true;
            } else {
                l.push(c);
                prev_space = false;
            }
        }
        let t = l.trim();
        if t.is_empty() {
            blanks += 1;
            continue;
        }
        if !first {
            out.push('\n');
            for _ in 0..blanks.min(max_blank) {
                out.push('\n');
            }
        }
        out.push_str(t);
        blanks = 0;
        first = false;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_and_composes() {
        let o = CleanOpts::default();
        assert_eq!(clean("ｆｕｌｌｗｉｄｔｈ １２３", &o), "fullwidth 123");
        assert_eq!(clean("e\u{0301}cole", &o), "école");
        assert_eq!(clean("a\u{030A}\u{0301}", &o), "\u{1FB}");
        assert_eq!(clean("ﬁne ﬂow", &o), "fine flow");
        assert_eq!(clean("a\u{00A0}\u{200B}b\u{FEFF}  c", &o), "a b c");
        // Arabic presentation forms fold back to base letters
        assert_eq!(clean("\u{FEE3}\u{FEAE}\u{FE92}\u{FE8E}", &o), "مربا");
        assert_eq!(clean("كـــتاب", &o), "كتاب");
        let mut o2 = CleanOpts::default();
        o2.strip_arabic_marks = true;
        o2.fold_arabic_letters = true;
        assert_eq!(clean("أَحْمَد", &o2), "احمد");
        assert_eq!(clean("a\r\n\r\n\r\n\r\nb  \n c", &o), "a\n\nb\nc");
    }
}
