//! Reproducible train/validation/test splitting: seeded, optionally stratified,
//! group-aware and duplicate-aware (duplicate clusters are kept together).

use crate::dedup;
use crate::json::Value;
use crate::text::{fnv1a, normalize, shingles, splitmix};
use std::collections::HashMap;

pub struct Item {
    pub id: String,
    pub label: Option<String>,
    pub group: Option<String>,
    pub text: Option<String>,
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        splitmix(self.0)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn shuffle<T>(v: &mut [T], rng: &mut Rng) {
    for i in (1..v.len()).rev() {
        let j = rng.below(i + 1);
        v.swap(i, j);
    }
}

pub fn run(items: &[Item], ratios: &[(String, f64)], seed: u64, dup_aware: bool, near_threshold: f64) -> Result<Value, String> {
    if ratios.is_empty() {
        return Err("no split ratios given".into());
    }
    let sum: f64 = ratios.iter().map(|r| r.1).sum();
    if ratios.iter().any(|r| r.1 < 0.0) || sum <= 0.0 {
        return Err("ratios must be non-negative and sum to > 0".into());
    }
    let n = items.len();

    // Build units: records that must stay together.
    let mut unit_of: Vec<usize> = vec![0; n];
    let mut key_to_unit: HashMap<String, usize> = HashMap::new();
    let mut cluster: Option<Vec<usize>> = None;
    if dup_aware {
        let norms: Vec<String> = items.iter().map(|i| normalize(i.text.as_deref().unwrap_or(""))).collect();
        let hashes: Vec<u64> = norms.iter().map(|t| if t.is_empty() { fnv1a(b"") ^ 0xFFFF } else { fnv1a(t.as_bytes()) }).collect();
        // Records with empty text must not be merged into one cluster: give them unique hashes.
        let hashes: Vec<u64> = hashes.iter().enumerate().map(|(i, h)| if norms[i].is_empty() { splitmix(i as u64 + 99) } else { *h }).collect();
        let exact = dedup::exact_groups(&hashes);
        let sets: Vec<Vec<u64>> = norms.iter().map(|t| shingles(t)).collect();
        let near = if near_threshold > 0.0 { dedup::near_pairs(&sets, near_threshold) } else { Vec::new() };
        cluster = Some(dedup::clusters(n, &exact, &near));
    }
    let mut units: Vec<Vec<usize>> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        let key = if let Some(g) = &it.group {
            format!("g:{}", g)
        } else if let Some(c) = &cluster {
            format!("c:{}", c[i])
        } else {
            format!("i:{}", i)
        };
        let u = *key_to_unit.entry(key).or_insert_with(|| {
            units.push(Vec::new());
            units.len() - 1
        });
        units[u].push(i);
        unit_of[i] = u;
    }
    // If a group and a duplicate cluster overlap, groups win (documented behaviour).

    // Stratification: unit label = label of its first member.
    let mut by_label: Vec<(String, Vec<usize>)> = Vec::new();
    let mut label_idx: HashMap<String, usize> = HashMap::new();
    for (u, members) in units.iter().enumerate() {
        let label = items[members[0]].label.clone().unwrap_or_default();
        let li = *label_idx.entry(label.clone()).or_insert_with(|| {
            by_label.push((label.clone(), Vec::new()));
            by_label.len() - 1
        });
        by_label[li].1.push(u);
    }
    by_label.sort_by(|a, b| a.0.cmp(&b.0));

    let mut assign: Vec<usize> = vec![0; n]; // index into ratios
    let mut totals: Vec<usize> = vec![0; ratios.len()];
    for (label, mut us) in by_label {
        let mut rng = Rng(seed ^ fnv1a(label.as_bytes()));
        shuffle(&mut us, &mut rng);
        let records_in_label: usize = us.iter().map(|u| units[*u].len()).sum();
        let target: Vec<f64> = ratios.iter().map(|r| records_in_label as f64 * r.1 / sum).collect();
        let mut got: Vec<usize> = vec![0; ratios.len()];
        for u in us {
            // Pick the split with the largest remaining deficit (stable tie-break by order).
            let mut best = 0usize;
            let mut best_def = f64::MIN;
            for (k, t) in target.iter().enumerate() {
                let def = *t - got[k] as f64;
                if def > best_def + 1e-12 {
                    best_def = def;
                    best = k;
                }
            }
            for &m in &units[u] {
                assign[m] = best;
            }
            got[best] += units[u].len();
            totals[best] += units[u].len();
        }
    }
    let _ = unit_of;

    let assignments: Vec<Value> = items
        .iter()
        .enumerate()
        .map(|(i, it)| Value::obj(vec![("id", Value::str(it.id.clone())), ("split", Value::str(ratios[assign[i]].0.clone()))]))
        .collect();
    let counts = Value::Obj(ratios.iter().enumerate().map(|(k, r)| (r.0.clone(), Value::int(totals[k] as i64))).collect());
    Ok(Value::obj(vec![
        ("assignments", Value::Arr(assignments)),
        ("counts", counts),
        ("seed", Value::int(seed as i64)),
        ("units", Value::int(units.len() as i64)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(n: usize) -> Vec<Item> {
        (0..n)
            .map(|i| Item { id: format!("r{}", i), label: Some(if i % 4 == 0 { "rare".into() } else { "common".into() }), group: None, text: Some(format!("unique sentence number {} alpha beta gamma delta {}", i, i * 31)) })
            .collect()
    }
    fn ratios() -> Vec<(String, f64)> {
        vec![("train".into(), 0.8), ("validation".into(), 0.1), ("test".into(), 0.1)]
    }
    fn split_of(v: &Value, id: &str) -> String {
        v.get("assignments").unwrap().as_arr().unwrap().iter().find(|a| a.get("id").unwrap().as_str().unwrap() == id).unwrap().get("split").unwrap().as_str().unwrap().to_string()
    }

    #[test]
    fn deterministic_and_proportional() {
        let it = items(1000);
        let a = run(&it, &ratios(), 42, false, 0.0).unwrap();
        let b = run(&it, &ratios(), 42, false, 0.0).unwrap();
        assert_eq!(a.to_json(), b.to_json());
        let c = run(&it, &ratios(), 43, false, 0.0).unwrap();
        assert_ne!(a.to_json(), c.to_json());
        let counts = a.get("counts").unwrap();
        assert!((counts.get("train").unwrap().as_f64().unwrap() - 800.0).abs() <= 3.0);
        assert!((counts.get("test").unwrap().as_f64().unwrap() - 100.0).abs() <= 3.0);
    }

    #[test]
    fn stratified_keeps_label_ratio() {
        let it = items(1000);
        let r = run(&it, &ratios(), 7, false, 0.0).unwrap();
        let rare_in_test = it.iter().filter(|i| i.label.as_deref() == Some("rare") && split_of(&r, &i.id) == "test").count();
        assert!((rare_in_test as i64 - 25).abs() <= 3, "rare in test = {}", rare_in_test);
    }

    #[test]
    fn groups_stay_together() {
        let mut it = items(300);
        for (i, x) in it.iter_mut().enumerate() {
            x.group = Some(format!("g{}", i / 5));
        }
        let r = run(&it, &ratios(), 1, false, 0.0).unwrap();
        for g in 0..60 {
            let s0 = split_of(&r, &format!("r{}", g * 5));
            for k in 1..5 {
                assert_eq!(split_of(&r, &format!("r{}", g * 5 + k)), s0);
            }
        }
    }

    #[test]
    fn duplicate_aware_keeps_duplicates_together() {
        let mut it = items(200);
        it[10].text = Some("the same text repeated for leakage test case".into());
        it[150].text = Some("the same text repeated for leakage test case".into());
        let r = run(&it, &ratios(), 5, true, 0.9).unwrap();
        assert_eq!(split_of(&r, "r10"), split_of(&r, "r150"));
    }
}
