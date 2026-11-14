//! Composition statistics computed from real record text.

use crate::json::Value;
use crate::text::{estimate_tokens, script_of};
use std::collections::BTreeMap;

pub fn run(texts: &[String]) -> Value {
    let n = texts.len();
    let mut lens: Vec<usize> = texts.iter().map(|t| t.chars().count()).collect();
    let mut scripts: BTreeMap<&'static str, i64> = BTreeMap::new();
    for t in texts {
        *scripts.entry(script_of(t)).or_insert(0) += 1;
    }
    let total: usize = lens.iter().sum();
    let mut toks: Vec<usize> = texts.iter().map(|t| estimate_tokens(t)).collect();
    let tok_total: usize = toks.iter().sum();
    toks.sort_unstable();
    let tok_p95 = if n == 0 { 0 } else { toks[(((n - 1) as f64) * 0.95).round() as usize] };
    lens.sort_unstable();
    let pct = |p: f64| -> i64 {
        if n == 0 {
            0
        } else {
            lens[(((n - 1) as f64) * p).round() as usize] as i64
        }
    };
    // Histogram with power-of-two buckets: <=16, <=32, ... , >4096.
    let edges = [16usize, 32, 64, 128, 256, 512, 1024, 2048, 4096];
    let mut buckets = vec![0i64; edges.len() + 1];
    for l in &lens {
        let mut placed = false;
        for (i, e) in edges.iter().enumerate() {
            if l <= e {
                buckets[i] += 1;
                placed = true;
                break;
            }
        }
        if !placed {
            *buckets.last_mut().unwrap() += 1;
        }
    }
    let labels: Vec<String> = edges.iter().map(|e| format!("<={}", e)).chain(std::iter::once(">4096".to_string())).collect();
    Value::obj(vec![
        ("count", Value::int(n as i64)),
        ("tokens_total", Value::int(tok_total as i64)),
        ("tokens_avg", Value::float(if n == 0 { 0.0 } else { tok_total as f64 / n as f64 })),
        ("tokens_max", Value::int(toks.last().copied().unwrap_or(0) as i64)),
        ("tokens_p95", Value::int(tok_p95 as i64)),
        ("total_chars", Value::int(total as i64)),
        ("avg_chars", Value::float(if n == 0 { 0.0 } else { total as f64 / n as f64 })),
        ("min_chars", Value::int(lens.first().copied().unwrap_or(0) as i64)),
        ("max_chars", Value::int(lens.last().copied().unwrap_or(0) as i64)),
        ("p50_chars", Value::int(pct(0.5))),
        ("p95_chars", Value::int(pct(0.95))),
        ("scripts", Value::Obj(scripts.into_iter().map(|(k, v)| (k.to_string(), Value::int(v))).collect())),
        (
            "length_histogram",
            Value::Arr(labels.into_iter().zip(buckets).map(|(l, c)| Value::obj(vec![("bucket", Value::str(l)), ("count", Value::int(c))])).collect()),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_basic() {
        let t = vec!["hello world".to_string(), "مرحبا بالعالم كيف الحال".to_string(), "x".repeat(300)];
        let s = run(&t);
        assert_eq!(s.get("count").unwrap().as_f64().unwrap(), 3.0);
        assert_eq!(s.get("scripts").unwrap().get("arabic").unwrap().as_f64().unwrap(), 1.0);
        assert_eq!(s.get("max_chars").unwrap().as_f64().unwrap(), 300.0);
        assert!(run(&[]).get("count").is_some());
    }
}
