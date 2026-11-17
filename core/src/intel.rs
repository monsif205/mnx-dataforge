//! Dataset intelligence: streaming accumulators for distributions (document/paragraph length, readability,
//! quality, language, source, license, domain, time) and the duplicate percentage. Memory: fixed histograms plus
//! one 64-bit hash per document for duplicate detection (tracking stops at 40M documents).

use crate::json::Value;
use crate::{nlp, text, web};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

const DOC_BINS: [u64; 11] = [10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000, u64::MAX];
const PARA_BINS: [u64; 8] = [20, 50, 100, 200, 400, 800, 1600, u64::MAX];
const LIX_BINS: [f64; 7] = [20.0, 30.0, 40.0, 50.0, 60.0, 70.0, f64::MAX];
const FLESCH_BINS: [f64; 7] = [0.0, 30.0, 50.0, 60.0, 70.0, 80.0, f64::MAX];
const MAX_TRACKED: usize = 40_000_000;
const SAMPLE_CHARS: usize = 20_000;

#[derive(Default)]
pub struct Intel {
    docs: u64,
    words_total: u64,
    chars_total: u64,
    doc_hist: [u64; 11],
    para_hist: [u64; 8],
    paras: u64,
    lix_hist: [u64; 7],
    flesch_hist: [u64; 7],
    flesch_n: u64,
    quality_hist: [u64; 10],
    quality_sum: f64,
    low_quality: u64,
    lang: HashMap<String, u64>,
    source: HashMap<String, u64>,
    license: HashMap<String, u64>,
    domain: HashMap<String, u64>,
    year: HashMap<String, u64>,
    month: HashMap<String, u64>,
    seen: HashSet<u64>,
    dups: u64,
    untracked: bool,
    empty: u64,
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn bin_u(bins: &[u64], v: u64) -> usize {
    bins.iter().position(|&b| v <= b).unwrap_or(bins.len() - 1)
}

fn bin_f(bins: &[f64], v: f64) -> usize {
    bins.iter().position(|&b| v < b).unwrap_or(bins.len() - 1)
}

fn bump(m: &mut HashMap<String, u64>, k: &str) {
    if m.len() < 50_000 || m.contains_key(k) {
        *m.entry(k.to_string()).or_insert(0) += 1;
    } else {
        *m.entry("(other)".into()).or_insert(0) += 1;
    }
}

/// Accepts "2024", "2024-05", "2024-05-01...", "2024/05/01", or a unix epoch in seconds/milliseconds.
fn year_month(d: &str) -> Option<(String, Option<String>)> {
    let t = d.trim();
    if t.chars().all(|c| c.is_ascii_digit()) && t.len() >= 9 {
        let mut n: i64 = t.parse().ok()?;
        if t.len() >= 12 {
            n /= 1000;
        }
        let days = n.div_euclid(86400);
        // civil-from-days (Howard Hinnant)
        let z = days + 719468;
        let era = z.div_euclid(146097);
        let doe = z.rem_euclid(146097);
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let mut y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        if m <= 2 {
            y += 1;
        }
        return Some((format!("{:04}", y), Some(format!("{:04}-{:02}", y, m))));
    }
    let b: Vec<char> = t.chars().collect();
    if b.len() >= 4 && b[..4].iter().all(|c| c.is_ascii_digit()) {
        let y: String = b[..4].iter().collect();
        let yi: i32 = y.parse().ok()?;
        if !(1900..=2100).contains(&yi) {
            return None;
        }
        if b.len() >= 7 && (b[4] == '-' || b[4] == '/') && b[5].is_ascii_digit() && b[6].is_ascii_digit() {
            let m: String = b[5..7].iter().collect();
            if m.parse::<u32>().map(|x| (1..=12).contains(&x)).unwrap_or(false) {
                return Some((y.clone(), Some(format!("{}-{}", y, m))));
            }
        }
        return Some((y, None));
    }
    None
}

impl Intel {
    pub fn add(&mut self, item: &Value) {
        let text_s = item.get("text").and_then(|t| t.as_str()).unwrap_or("");
        self.docs += 1;
        if text_s.trim().is_empty() {
            self.empty += 1;
        }
        let nwords = nlp::words(text_s).len() as u64;
        self.words_total += nwords;
        self.chars_total += text_s.chars().count() as u64;
        self.doc_hist[bin_u(&DOC_BINS, nwords)] += 1;
        for p in text_s.split("\n\n").map(|p| p.trim()).filter(|p| !p.is_empty()) {
            self.paras += 1;
            self.para_hist[bin_u(&PARA_BINS, p.chars().count() as u64)] += 1;
        }
        let sample: String = text_s.chars().take(SAMPLE_CHARS).collect();
        let (lix, flesch, _, _) = nlp::readability(&sample);
        if nwords >= 5 {
            self.lix_hist[bin_f(&LIX_BINS, lix)] += 1;
            if let Some(f) = flesch {
                self.flesch_hist[bin_f(&FLESCH_BINS, f)] += 1;
                self.flesch_n += 1;
            }
        }
        let q = item.get("quality").and_then(|x| x.as_f64()).unwrap_or_else(|| nlp::quality(&sample).0);
        self.quality_sum += q;
        self.quality_hist[((q / 10.0) as usize).min(9)] += 1;
        if q < 50.0 {
            self.low_quality += 1;
        }
        let lang = item.get("lang").and_then(|x| x.as_str()).map(|s| s.to_string()).unwrap_or_else(|| nlp::detect_lang(&sample).0);
        bump(&mut self.lang, &lang);
        bump(&mut self.source, item.get("source").and_then(|x| x.as_str()).filter(|s| !s.is_empty()).unwrap_or("(unknown)"));
        bump(&mut self.license, item.get("license").and_then(|x| x.as_str()).filter(|s| !s.is_empty()).unwrap_or("(none)"));
        let dom = item.get("url").and_then(|x| x.as_str()).and_then(web::domain_of).unwrap_or_else(|| "(none)".into());
        bump(&mut self.domain, &dom);
        match item.get("date").and_then(|x| x.as_str().map(|s| s.to_string()).or_else(|| x.as_f64().map(|f| format!("{}", f as i64)))).and_then(|d| year_month(&d)) {
            Some((y, m)) => {
                bump(&mut self.year, &y);
                if let Some(m) = m {
                    bump(&mut self.month, &m);
                }
            }
            None => bump(&mut self.year, "(unknown)"),
        }
        if !text_s.trim().is_empty() {
            if self.seen.len() >= MAX_TRACKED {
                self.untracked = true;
            } else if !self.seen.insert(fnv(&text::normalize(text_s))) {
                self.dups += 1;
            }
        }
    }

    fn pct(n: u64, total: u64) -> f64 {
        if total == 0 { 0.0 } else { (n as f64 * 10000.0 / total as f64).round() / 100.0 }
    }

    fn hist(labels: &[String], counts: &[u64]) -> Value {
        let total: u64 = counts.iter().sum();
        Value::Arr(labels.iter().zip(counts).map(|(l, &c)| Value::obj(vec![("label", Value::str(l.clone())), ("count", Value::int(c as i64)), ("pct", Value::float(Self::pct(c, total)))])).collect())
    }

    fn top(m: &HashMap<String, u64>, n: usize, sort_by_key: bool) -> Value {
        let total: u64 = m.values().sum();
        let mut v: Vec<(&String, &u64)> = m.iter().collect();
        if sort_by_key {
            v.sort_by(|a, b| a.0.cmp(b.0));
        } else {
            v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        }
        let mut out: Vec<Value> = Vec::new();
        let mut shown = 0u64;
        for (k, c) in v.iter().take(n) {
            shown += **c;
            out.push(Value::obj(vec![("label", Value::str((*k).clone())), ("count", Value::int(**c as i64)), ("pct", Value::float(Self::pct(**c, total)))]));
        }
        if total > shown {
            out.push(Value::obj(vec![("label", Value::str("(rest)")), ("count", Value::int((total - shown) as i64)), ("pct", Value::float(Self::pct(total - shown, total)))]));
        }
        Value::Arr(out)
    }

    pub fn report(&self) -> Value {
        let ranges_u = |bins: &[u64]| -> Vec<String> {
            let mut prev = 0u64;
            bins.iter()
                .map(|&b| {
                    let l = if b == u64::MAX { format!("{}+", prev) } else { format!("{}-{}", prev, b) };
                    prev = b.saturating_add(1);
                    l
                })
                .collect()
        };
        let ranges_f = |bins: &[f64]| -> Vec<String> {
            let mut prev = f64::MIN;
            bins.iter()
                .map(|&b| {
                    let l = if prev == f64::MIN { format!("<{}", b) } else if b == f64::MAX { format!("{}+", prev) } else { format!("{}-{}", prev, b) };
                    prev = b;
                    l
                })
                .collect()
        };
        let qlabels: Vec<String> = (0..10).map(|i| format!("{}-{}", i * 10, i * 10 + 10)).collect();
        let tracked = self.docs - self.empty;
        Value::obj(vec![
            ("documents", Value::int(self.docs as i64)),
            ("empty_documents", Value::int(self.empty as i64)),
            ("words", Value::int(self.words_total as i64)),
            ("characters", Value::int(self.chars_total as i64)),
            ("paragraphs", Value::int(self.paras as i64)),
            ("avg_words_per_document", Value::float(if self.docs > 0 { (self.words_total as f64 / self.docs as f64 * 10.0).round() / 10.0 } else { 0.0 })),
            ("duplicate_documents", Value::int(self.dups as i64)),
            ("duplicate_percent", Value::float(Self::pct(self.dups, tracked))),
            ("duplicate_tracking_complete", Value::Bool(!self.untracked)),
            ("avg_quality", Value::float(if self.docs > 0 { (self.quality_sum / self.docs as f64 * 10.0).round() / 10.0 } else { 0.0 })),
            ("low_quality_percent", Value::float(Self::pct(self.low_quality, self.docs))),
            ("document_length_words", Self::hist(&ranges_u(&DOC_BINS), &self.doc_hist)),
            ("paragraph_length_chars", Self::hist(&ranges_u(&PARA_BINS), &self.para_hist)),
            ("readability_lix", Self::hist(&ranges_f(&LIX_BINS), &self.lix_hist)),
            ("readability_flesch", Self::hist(&ranges_f(&FLESCH_BINS), &self.flesch_hist)),
            ("quality", Self::hist(&qlabels, &self.quality_hist)),
            ("language", Self::top(&self.lang, 20, false)),
            ("source", Self::top(&self.source, 20, false)),
            ("license", Self::top(&self.license, 20, false)),
            ("domain", Self::top(&self.domain, 20, false)),
            ("year", Self::top(&self.year, 60, true)),
            ("month", Self::top(&self.month, 240, true)),
        ])
    }
}

static REG: OnceLock<Mutex<(u64, HashMap<u64, Intel>)>> = OnceLock::new();

fn reg() -> &'static Mutex<(u64, HashMap<u64, Intel>)> {
    REG.get_or_init(|| Mutex::new((0, HashMap::new())))
}

fn handle(v: &Value) -> Result<u64, String> {
    v.get("handle").and_then(|h| h.as_f64()).map(|h| h as u64).ok_or_else(|| "missing 'handle'".to_string())
}

pub fn open(_v: &Value) -> Result<Value, String> {
    let mut g = reg().lock().map_err(|_| "registry poisoned".to_string())?;
    g.0 += 1;
    let id = g.0;
    g.1.insert(id, Intel::default());
    Ok(Value::obj(vec![("handle", Value::int(id as i64))]))
}

pub fn add(v: &Value) -> Result<Value, String> {
    let h = handle(v)?;
    let items = v.get("items").and_then(|a| a.as_arr()).ok_or("missing 'items'")?;
    let mut g = reg().lock().map_err(|_| "registry poisoned".to_string())?;
    let acc = g.1.get_mut(&h).ok_or("unknown handle")?;
    for it in items {
        acc.add(it);
    }
    Ok(Value::obj(vec![("documents", Value::int(acc.docs as i64))]))
}

pub fn finish(v: &Value) -> Result<Value, String> {
    let h = handle(v)?;
    let mut g = reg().lock().map_err(|_| "registry poisoned".to_string())?;
    let acc = g.1.remove(&h).ok_or("unknown handle")?;
    Ok(acc.report())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    #[test]
    fn report_shape_and_duplicates() {
        let items = r#"{"items":[
          {"text":"The quick brown fox jumps over the lazy dog. It is a long enough sentence for scoring.","source":"a","license":"MIT","url":"https://www.x.com/a","date":"2024-03-05"},
          {"text":"The quick brown fox jumps over the lazy dog. It is a long enough sentence for scoring.","source":"a","license":"MIT","url":"https://x.com/b","date":"2024-04-01"},
          {"text":"Bonjour le monde, ceci est une phrase assez longue pour le test de la langue.","source":"b","license":"CC0-1.0","date":"2023"},
          {"text":"","source":"b"}]}"#;
        let o = open(&Value::Null).unwrap();
        let h = o.get("handle").unwrap().as_f64().unwrap();
        let mut req = parse(items).unwrap();
        req.set("handle", Value::float(h));
        add(&req).unwrap();
        let r = finish(&parse(&format!("{{\"handle\":{}}}", h)).unwrap()).unwrap();
        assert_eq!(r.get("documents").unwrap().as_f64().unwrap(), 4.0);
        assert_eq!(r.get("duplicate_documents").unwrap().as_f64().unwrap(), 1.0);
        assert!((r.get("duplicate_percent").unwrap().as_f64().unwrap() - 33.33).abs() < 0.01);
        let dom = r.get("domain").unwrap().as_arr().unwrap();
        let x = dom.iter().find(|d| d.get("label").unwrap().as_str().unwrap() == "x.com").unwrap();
        assert_eq!(x.get("count").unwrap().as_f64().unwrap(), 2.0);
        let langs: Vec<&str> = r.get("language").unwrap().as_arr().unwrap().iter().map(|x| x.get("label").unwrap().as_str().unwrap()).collect();
        assert!(langs.contains(&"en") && langs.contains(&"fr"));
        let months = r.get("month").unwrap().as_arr().unwrap();
        assert_eq!(months.len(), 2);
        assert!(finish(&parse(&format!("{{\"handle\":{}}}", h)).unwrap()).is_err());
        assert_eq!(year_month("1714521600").unwrap().1.unwrap(), "2024-05");
        assert_eq!(year_month("1714521600000").unwrap().0, "2024");
    }
}
