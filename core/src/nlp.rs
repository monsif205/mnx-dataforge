//! Lightweight text analysis: sentence boundaries, paragraph reflow, language guess, readability, quality score.
//! Rule based and allocation-light; works for Latin, Arabic and CJK text.

use crate::text::script_of;

const ABBR: &[&str] = &[
    "mr", "mrs", "ms", "dr", "prof", "sr", "jr", "st", "vs", "etc", "e.g", "i.e", "fig", "no", "vol", "inc", "ltd", "co", "corp", "mt", "jan", "feb", "mar", "apr", "jun", "jul", "aug", "sep", "sept", "oct", "nov", "dec", "approx", "dept",
    "est", "al", "cf", "ed", "pp", "mme", "mlle", "bd", "av", "gen", "col", "lt", "sgt", "rev", "hon", "u.s", "u.k",
];

fn is_term(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '…' | '。' | '！' | '？' | '؟' | '۔' | '।')
}

fn is_closer(c: char) -> bool {
    matches!(c, '"' | '\'' | ')' | ']' | '}' | '»' | '”' | '’' | '›' | '」' | '』' | '）')
}

/// Splits text into sentences. Newline-separated paragraphs always end a sentence.
pub fn sentences(text: &str) -> Vec<String> {
    let ch: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < ch.len() {
        let c = ch[i];
        if c == '\n' {
            // a blank line or a line ending with a terminator closes the sentence; a lone newline is a soft wrap
            let blank = i + 1 < ch.len() && ch[i + 1] == '\n';
            let ended = cur.trim_end().chars().last().map(|x| is_term(x) || is_closer(x)).unwrap_or(false);
            if blank || ended {
                push(&mut out, &mut cur);
            } else {
                cur.push(' ');
            }
            i += 1;
            continue;
        }
        cur.push(c);
        if is_term(c) {
            let mut j = i + 1;
            while j < ch.len() && (is_term(ch[j]) || is_closer(ch[j])) {
                cur.push(ch[j]);
                j += 1;
            }
            let next = ch.get(j).copied();
            let cjk = matches!(c, '。' | '！' | '？' | '।' | '۔');
            let boundary = match next {
                None => true,
                Some(n) if n.is_whitespace() => {
                    let mut k = j;
                    while k < ch.len() && ch[k].is_whitespace() && ch[k] != '\n' {
                        k += 1;
                    }
                    let after = ch.get(k).copied();
                    if c == '.' && !dot_boundary(&cur, after) {
                        false
                    } else if (c == '!' || c == '?') && after.map(|a| a.is_lowercase()).unwrap_or(false) {
                        false
                    } else {
                        true
                    }
                }
                Some(_) => cjk,
            };
            if boundary {
                push(&mut out, &mut cur);
            }
            i = j;
            continue;
        }
        i += 1;
    }
    push(&mut out, &mut cur);
    out
}

fn push(out: &mut Vec<String>, cur: &mut String) {
    let t = cur.trim();
    if !t.is_empty() {
        out.push(t.to_string());
    }
    cur.clear();
}

/// Decides whether a '.' that is followed by whitespace really ends a sentence.
fn dot_boundary(cur: &str, after: Option<char>) -> bool {
    let body = cur.trim_end_matches(|c: char| is_closer(c) || c == '.');
    let word: String = body.chars().rev().take_while(|c| !c.is_whitespace()).collect::<Vec<_>>().into_iter().rev().collect();
    let w = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '.').to_lowercase();
    if cur.ends_with("..") || cur.ends_with("...") {
        return after.map(|a| a.is_uppercase() || !a.is_alphabetic()).unwrap_or(true);
    }
    if ABBR.contains(&w.as_str()) {
        return false;
    }
    let letters: Vec<char> = w.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() == 1 && w.chars().count() <= 3 && cur.trim_end().chars().rev().nth(1).map(|c| c.is_uppercase() || c == '.').unwrap_or(false) {
        return false; // initials such as "J." or "U.S."
    }
    if after.map(|a| a.is_lowercase()).unwrap_or(false) {
        return false;
    }
    true
}

/// Joins hard-wrapped lines back into paragraphs (plain text, PDF extracts). Blank lines separate paragraphs;
/// a line break is kept when the line ends a sentence or the next line looks like a list item or heading.
pub fn reflow(text: &str) -> String {
    let mut paras: Vec<String> = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, paras: &mut Vec<String>| {
        if !cur.trim().is_empty() {
            paras.push(cur.trim().to_string());
        }
        cur.clear();
    };
    for raw in text.replace('\r', "").split('\n') {
        let line = raw.trim();
        if line.is_empty() {
            flush(&mut cur, &mut paras);
            continue;
        }
        let list_like = line.starts_with("- ") || line.starts_with("* ") || line.starts_with("• ") || line.chars().take(4).any(|c| c == '.' || c == ')') && line.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false);
        if cur.is_empty() {
            cur.push_str(line);
            continue;
        }
        if list_like || line.starts_with('#') {
            flush(&mut cur, &mut paras);
            cur.push_str(line);
            continue;
        }
        let last = cur.chars().last().unwrap();
        let first = line.chars().next().unwrap();
        if last == '-' && cur.chars().rev().nth(1).map(|c| c.is_alphabetic()).unwrap_or(false) && first.is_lowercase() {
            cur.pop(); // dehyphenate "inter-\nnational"
            cur.push_str(line);
        } else if is_term(last) || is_closer(last) && cur.chars().rev().nth(1).map(is_term).unwrap_or(false) {
            if first.is_uppercase() || !first.is_alphabetic() {
                cur.push('\n');
            } else {
                cur.push(' ');
            }
            cur.push_str(line);
        } else {
            cur.push(' ');
            cur.push_str(line);
        }
    }
    flush(&mut cur, &mut paras);
    paras.join("\n\n")
}

pub fn words(text: &str) -> Vec<&str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '’' || c == '-')).filter(|w| !w.is_empty()).collect()
}

const STOP: &[(&str, &[&str])] = &[
    ("en", &["the", "and", "of", "to", "in", "is", "that", "it", "for", "with", "as", "was", "on", "are", "this", "be", "by", "at", "or", "from", "have", "not", "you", "an"]),
    ("fr", &["le", "la", "les", "des", "et", "est", "un", "une", "que", "pour", "dans", "qui", "sur", "pas", "au", "avec", "ce", "il", "elle", "du", "en", "sont", "plus"]),
    ("es", &["el", "la", "los", "las", "de", "que", "y", "en", "un", "una", "es", "por", "con", "para", "del", "se", "no", "su", "al", "lo", "como", "más"]),
    ("de", &["der", "die", "und", "das", "ist", "nicht", "ein", "eine", "zu", "mit", "den", "von", "auf", "für", "sich", "dem", "auch", "es", "im", "wird"]),
    ("it", &["il", "di", "che", "la", "e", "per", "un", "una", "non", "con", "sono", "del", "della", "le", "si", "da", "in", "più", "anche", "come"]),
    ("pt", &["o", "a", "de", "que", "e", "do", "da", "em", "um", "uma", "para", "com", "não", "os", "as", "se", "por", "mais", "dos", "como"]),
    ("nl", &["de", "het", "een", "en", "van", "is", "dat", "op", "te", "in", "niet", "met", "voor", "zijn", "er", "aan", "ook", "als"]),
];

/// Returns (language code, confidence 0..1). Codes are ISO 639-1 where known; otherwise the script name.
pub fn detect_lang(text: &str) -> (String, f64) {
    let sample: String = text.chars().take(4000).collect();
    let (mut ar, mut lat, mut cjk, mut kana, mut hang, mut cyr, mut grk, mut heb, mut dev, mut tot) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    for c in sample.chars().filter(|c| c.is_alphabetic()) {
        tot += 1;
        match c as u32 {
            0x600..=0x6FF | 0x750..=0x77F | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => ar += 1,
            0x3040..=0x30FF => kana += 1,
            0x4E00..=0x9FFF | 0x3400..=0x4DBF => cjk += 1,
            0xAC00..=0xD7AF => hang += 1,
            0x400..=0x4FF => cyr += 1,
            0x370..=0x3FF => grk += 1,
            0x590..=0x5FF => heb += 1,
            0x900..=0x97F => dev += 1,
            0x41..=0x24F => lat += 1,
            _ => {}
        }
    }
    if tot == 0 {
        return ("und".into(), 0.0);
    }
    let share = |n: usize| n as f64 / tot as f64;
    let script_pick: [(&str, usize); 8] = [("ar", ar), ("ja", kana), ("zh", cjk), ("ko", hang), ("cyrillic", cyr), ("el", grk), ("he", heb), ("hi", dev)];
    let (name, n) = script_pick.iter().copied().max_by_key(|x| x.1).unwrap();
    if n > lat && share(n) > 0.3 {
        let nm = if name == "zh" && kana > 0 { "ja" } else { name };
        return (nm.to_string(), share(n).min(1.0));
    }
    if share(lat) < 0.3 {
        return ("und".into(), 0.0);
    }
    let lower = sample.to_lowercase();
    let ws: Vec<&str> = words(&lower);
    if ws.len() < 3 {
        return ("und".into(), 0.1);
    }
    let mut best = ("und", 0.0f64);
    let mut second = 0.0f64;
    for (code, list) in STOP {
        let hits = ws.iter().filter(|w| list.contains(w)).count() as f64 / ws.len() as f64;
        if hits > best.1 {
            second = best.1;
            best = (code, hits);
        } else if hits > second {
            second = hits;
        }
    }
    if best.1 < 0.06 {
        return ("und".into(), best.1);
    }
    (best.0.to_string(), ((best.1 - second) / best.1.max(0.01)).clamp(0.0, 1.0) * 0.5 + 0.5)
}

fn syllables(w: &str) -> usize {
    let mut n = 0;
    let mut prev = false;
    for c in w.to_lowercase().chars() {
        let v = matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y' | 'é' | 'è' | 'à' | 'â' | 'ê' | 'î' | 'ô' | 'û' | 'ù' | 'á' | 'í' | 'ó' | 'ú' | 'ä' | 'ö' | 'ü');
        if v && !prev {
            n += 1;
        }
        prev = v;
    }
    if w.to_lowercase().ends_with('e') && n > 1 {
        n -= 1;
    }
    n.max(1)
}

/// (LIX, Flesch reading ease or None for non-Latin text, avg words per sentence, avg word length)
pub fn readability(text: &str) -> (f64, Option<f64>, f64, f64) {
    let sents = sentences(text);
    let ws = words(text);
    if ws.is_empty() || sents.is_empty() {
        return (0.0, None, 0.0, 0.0);
    }
    let nw = ws.len() as f64;
    let ns = sents.len().max(1) as f64;
    let long = ws.iter().filter(|w| w.chars().count() > 6).count() as f64;
    let lix = nw / ns + 100.0 * long / nw;
    let avg_len = ws.iter().map(|w| w.chars().count()).sum::<usize>() as f64 / nw;
    let latin = script_of(text) == "latin";
    let flesch = if latin {
        let syl: usize = ws.iter().map(|w| syllables(w)).sum();
        Some(206.835 - 1.015 * (nw / ns) - 84.6 * (syl as f64 / nw))
    } else {
        None
    };
    (lix, flesch, nw / ns, avg_len)
}

/// Heuristic quality score 0..100 with the reasons that lowered it.
pub fn quality(text: &str) -> (f64, Vec<&'static str>) {
    let mut score = 100.0f64;
    let mut why: Vec<&'static str> = Vec::new();
    let ws = words(text);
    let n = ws.len();
    if n < 5 {
        score -= 70.0;
        why.push("very short");
    } else if n < 20 {
        score -= 35.0;
        why.push("short");
    }
    let nonws = text.chars().filter(|c| !c.is_whitespace()).count().max(1) as f64;
    let alpha = text.chars().filter(|c| c.is_alphabetic()).count() as f64 / nonws;
    if alpha < 0.6 {
        score -= 30.0;
        why.push("low letter ratio");
    }
    let digits = text.chars().filter(|c| c.is_ascii_digit()).count() as f64 / nonws;
    if digits > 0.3 {
        score -= 15.0;
        why.push("mostly digits");
    }
    if n > 0 {
        let avg = ws.iter().map(|w| w.chars().count()).sum::<usize>() as f64 / n as f64;
        if !(2.5..=12.0).contains(&avg) {
            score -= 20.0;
            why.push("odd word length");
        }
    }
    let letters: Vec<char> = text.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() > 20 {
        let up = letters.iter().filter(|c| c.is_uppercase()).count() as f64 / letters.len() as f64;
        if up > 0.5 {
            score -= 15.0;
            why.push("mostly uppercase");
        }
    }
    let lines: Vec<&str> = text.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    if lines.len() >= 4 {
        let ended = lines.iter().filter(|l| l.chars().last().map(is_term).unwrap_or(false)).count() as f64 / lines.len() as f64;
        if ended < 0.1 {
            score -= 15.0;
            why.push("lines rarely end like sentences");
        }
        let mut seen = std::collections::HashSet::new();
        let dup = lines.iter().filter(|l| !seen.insert(**l)).count() as f64 / lines.len() as f64;
        if dup > 0.3 {
            score -= 25.0;
            why.push("many repeated lines");
        }
    }
    if n > 50 {
        let uniq: std::collections::HashSet<String> = ws.iter().map(|w| w.to_lowercase()).collect();
        if (uniq.len() as f64 / n as f64) < 0.2 {
            score -= 20.0;
            why.push("very repetitive");
        }
    }
    (score.clamp(0.0, 100.0), why)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentence_rules() {
        let s = sentences("Dr. Smith went to Washington. He met J. Brown at 3.5 p.m. It was fine! Really? yes. Done.");
        assert_eq!(s.len(), 5, "{:?}", s);
        assert_eq!(s[0], "Dr. Smith went to Washington.");
        let s = sentences("مرحبا بك. كيف حالك؟ أنا بخير.");
        assert_eq!(s.len(), 3, "{:?}", s);
        let s = sentences("今日は晴れです。明日は雨です。");
        assert_eq!(s.len(), 2);
        let s = sentences("Visit www.example.com for more. Thanks.");
        assert_eq!(s.len(), 2, "{:?}", s);
        let s = sentences("He said \"stop.\" Then he left.");
        assert_eq!(s.len(), 2, "{:?}", s);
        let s = sentences("first line\nsecond line continues.\n\nNew paragraph.");
        assert_eq!(s.len(), 2, "{:?}", s);
    }

    #[test]
    fn reflow_joins_wrapped_lines() {
        let t = "This is a long sentence that was wrapped\nby a PDF extractor and inter-\nnational words.\nNext sentence starts here.\n\nSecond paragraph.";
        let r = reflow(t);
        assert_eq!(r, "This is a long sentence that was wrapped by a PDF extractor and international words.\nNext sentence starts here.\n\nSecond paragraph.");
    }

    #[test]
    fn language_and_quality() {
        assert_eq!(detect_lang("The quick brown fox jumps over the lazy dog and it is on the road to the city").0, "en");
        assert_eq!(detect_lang("Le chat est sur la table et il mange une souris dans la maison avec les enfants").0, "fr");
        assert_eq!(detect_lang("مرحبا بالعالم كيف الحال اليوم").0, "ar");
        assert_eq!(detect_lang("これは日本語の文章です。今日は天気がいいです。").0, "ja");
        let (q, _) = quality("ok");
        assert!(q < 40.0);
        let good = "This is a reasonably long and well formed paragraph of text. It has several sentences, normal words and proper punctuation, which should score high on every check we apply to it. Nothing is repeated here.";
        assert!(quality(good).0 > 85.0);
        let (lix, fl, _, _) = readability(good);
        assert!(lix > 10.0 && fl.is_some());
    }
}
