//! Train/evaluation leakage detection. Exact duplicates are deterministic findings;
//! near duplicates and shared sources are heuristic/informational. Records listed in
//! `intended` are reported as intended similarity, not leakage.

use crate::dedup;
use crate::json::Value;
use crate::text::{fnv1a, normalize, shingles};
use std::collections::{HashMap, HashSet};

pub struct Rec {
    pub id: String,
    pub split: String,
    pub text: String,
    pub source: Option<String>,
}

pub fn run(recs: &[Rec], eval_splits: &[String], threshold: f64, intended: &HashSet<String>) -> Value {
    let norms: Vec<String> = recs.iter().map(|r| normalize(&r.text)).collect();
    let is_eval = |s: &str| eval_splits.iter().any(|e| e == s);
    let mut out: Vec<Value> = Vec::new();
    let mut leak_count = 0i64;
    let mut intended_count = 0i64;

    let push = |a: usize, b: usize, kind: &str, det: bool, sim: f64, out: &mut Vec<Value>, leak: &mut i64, int: &mut i64| {
        let (ra, rb) = (&recs[a], &recs[b]);
        let intended_flag = intended.contains(&ra.id) || intended.contains(&rb.id);
        if intended_flag {
            *int += 1;
        } else {
            *leak += 1;
        }
        out.push(Value::obj(vec![
            ("a", Value::str(ra.id.clone())),
            ("a_split", Value::str(ra.split.clone())),
            ("b", Value::str(rb.id.clone())),
            ("b_split", Value::str(rb.split.clone())),
            ("kind", Value::str(kind)),
            ("deterministic", Value::Bool(det)),
            ("similarity", Value::float(sim)),
            ("intended", Value::Bool(intended_flag)),
        ]));
    };

    // Exact duplicates across different splits where at least one side is an evaluation split.
    let hashes: Vec<u64> = norms.iter().enumerate().map(|(i, t)| if t.is_empty() { fnv1a(format!("empty{}", i).as_bytes()) } else { fnv1a(t.as_bytes()) }).collect();
    let mut exact_pairs: HashSet<(usize, usize)> = HashSet::new();
    for g in dedup::exact_groups(&hashes) {
        for x in 0..g.len() {
            for y in (x + 1)..g.len() {
                let (a, b) = (g[x], g[y]);
                if recs[a].split != recs[b].split && (is_eval(&recs[a].split) || is_eval(&recs[b].split)) {
                    exact_pairs.insert((a, b));
                    push(a, b, "exact_duplicate", true, 1.0, &mut out, &mut leak_count, &mut intended_count);
                }
            }
        }
    }
    if threshold > 0.0 {
        let sets: Vec<Vec<u64>> = norms.iter().map(|t| shingles(t)).collect();
        for (a, b, j) in dedup::near_pairs(&sets, threshold) {
            if recs[a].split != recs[b].split && (is_eval(&recs[a].split) || is_eval(&recs[b].split)) && !exact_pairs.contains(&(a, b)) {
                push(a, b, "near_duplicate", false, j, &mut out, &mut leak_count, &mut intended_count);
            }
        }
    }
    // Shared source records across splits (informational heuristic).
    let mut by_source: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, r) in recs.iter().enumerate() {
        if let Some(s) = &r.source {
            if !s.is_empty() {
                by_source.entry(s.as_str()).or_default().push(i);
            }
        }
    }
    let mut shared_sources = 0i64;
    for (_, members) in by_source {
        let splits: HashSet<&str> = members.iter().map(|m| recs[*m].split.as_str()).collect();
        if splits.len() > 1 && splits.iter().any(|s| is_eval(s)) {
            shared_sources += 1;
            let first_eval = members.iter().find(|m| is_eval(&recs[**m].split));
            let first_other = members.iter().find(|m| !is_eval(&recs[**m].split));
            if let (Some(a), Some(b)) = (first_eval, first_other) {
                push(*a, *b, "shared_source", false, 0.0, &mut out, &mut leak_count, &mut intended_count);
            }
        }
    }
    Value::obj(vec![
        ("records", Value::int(recs.len() as i64)),
        ("potential_leaks", Value::int(leak_count)),
        ("intended_similarity", Value::int(intended_count)),
        ("shared_sources", Value::int(shared_sources)),
        ("findings", Value::Arr(out)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(id: &str, split: &str, text: &str, src: Option<&str>) -> Rec {
        Rec { id: id.into(), split: split.into(), text: text.into(), source: src.map(|s| s.to_string()) }
    }

    #[test]
    fn detects_cross_split_duplicates() {
        let recs = vec![
            r("a", "train", "What is the capital of Morocco? Rabat.", Some("s1")),
            r("b", "test", "what is the capital of morocco rabat", Some("s2")),
            r("c", "train", "Completely unrelated cooking text about tagine and couscous preparation steps", Some("s3")),
            r("d", "train", "same train twin", None),
            r("e", "train", "same train twin", None),
        ];
        let rep = run(&recs, &["test".to_string()], 0.8, &HashSet::new());
        assert_eq!(rep.get("potential_leaks").unwrap().as_f64().unwrap(), 1.0);
        let f = &rep.get("findings").unwrap().as_arr().unwrap()[0];
        assert_eq!(f.get("kind").unwrap().as_str().unwrap(), "exact_duplicate");
    }

    #[test]
    fn intended_is_not_leakage() {
        let recs = vec![r("a", "train", "same text here", None), r("b", "test", "same text here", None)];
        let mut ok = HashSet::new();
        ok.insert("b".to_string());
        let rep = run(&recs, &["test".to_string()], 0.8, &ok);
        assert_eq!(rep.get("potential_leaks").unwrap().as_f64().unwrap(), 0.0);
        assert_eq!(rep.get("intended_similarity").unwrap().as_f64().unwrap(), 1.0);
    }
}
