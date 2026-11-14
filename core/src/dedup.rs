//! Exact and near-duplicate detection (MinHash + LSH banding, verified by exact Jaccard).
//! Near-duplicate detection is a HEURISTIC: it finds textually similar records, it does not
//! prove they are semantically the same.

use crate::text::{jaccard, splitmix};
use std::collections::{HashMap, HashSet};

const K: usize = 32;
const BANDS: usize = 8;
const ROWS: usize = 4;
const EXACT_LIMIT: usize = 400; // below this size compare all pairs exactly
const MAX_BUCKET: usize = 256;
const MAX_PAIRS: usize = 2_000_000;

/// Groups of indices that share the same hash (only groups with 2+ members).
pub fn exact_groups(hashes: &[u64]) -> Vec<Vec<usize>> {
    let mut map: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, h) in hashes.iter().enumerate() {
        map.entry(*h).or_default().push(i);
    }
    let mut groups: Vec<Vec<usize>> = map.into_values().filter(|g| g.len() > 1).collect();
    groups.sort_by_key(|g| g[0]);
    groups
}

fn signature(set: &[u64]) -> [u64; K] {
    let mut sig = [u64::MAX; K];
    for s in set {
        for (i, slot) in sig.iter_mut().enumerate() {
            let h = splitmix(*s ^ splitmix(i as u64 + 1));
            if h < *slot {
                *slot = h;
            }
        }
    }
    sig
}

/// Pairs (a, b, jaccard) with a < b and jaccard >= threshold. Sorted by (a, b).
pub fn near_pairs(sets: &[Vec<u64>], threshold: f64) -> Vec<(usize, usize, f64)> {
    let n = sets.len();
    let mut out: Vec<(usize, usize, f64)> = Vec::new();
    if n < 2 {
        return out;
    }
    if n <= EXACT_LIMIT {
        for a in 0..n {
            if sets[a].is_empty() {
                continue;
            }
            for b in (a + 1)..n {
                let j = jaccard(&sets[a], &sets[b]);
                if j >= threshold {
                    out.push((a, b, j));
                }
            }
        }
        return out;
    }
    let sigs: Vec<Option<[u64; K]>> = sets.iter().map(|s| if s.is_empty() { None } else { Some(signature(s)) }).collect();
    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    for band in 0..BANDS {
        let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
        for (i, sig) in sigs.iter().enumerate() {
            if let Some(sig) = sig {
                let mut key = splitmix(band as u64 + 7);
                for r in 0..ROWS {
                    key = splitmix(key ^ sig[band * ROWS + r]);
                }
                buckets.entry(key).or_default().push(i);
            }
        }
        for (_, members) in buckets {
            if members.len() < 2 {
                continue;
            }
            let m = &members[..members.len().min(MAX_BUCKET)];
            for x in 0..m.len() {
                for y in (x + 1)..m.len() {
                    if seen.len() >= MAX_PAIRS {
                        break;
                    }
                    seen.insert((m[x].min(m[y]), m[x].max(m[y])));
                }
            }
        }
    }
    for (a, b) in seen {
        let j = jaccard(&sets[a], &sets[b]);
        if j >= threshold {
            out.push((a, b, j));
        }
    }
    out.sort_by(|x, y| (x.0, x.1).cmp(&(y.0, y.1)));
    out
}

/// Union-find cluster ids (root index per element) from exact groups and near pairs.
pub fn clusters(n: usize, exact: &[Vec<usize>], near: &[(usize, usize, f64)]) -> Vec<usize> {
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut Vec<usize>, x: usize) -> usize {
        let mut r = x;
        while p[r] != r {
            r = p[r];
        }
        let mut c = x;
        while p[c] != r {
            let nx = p[c];
            p[c] = r;
            c = nx;
        }
        r
    }
    let union = |a: usize, b: usize, p: &mut Vec<usize>| {
        let (ra, rb) = (find(p, a), find(p, b));
        if ra != rb {
            p[ra.max(rb)] = ra.min(rb);
        }
    };
    for g in exact {
        for w in g.windows(2) {
            union(w[0], w[1], &mut parent);
        }
    }
    for (a, b, _) in near {
        union(*a, *b, &mut parent);
    }
    (0..n).map(|i| find(&mut parent, i)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{normalize, shingles};

    #[test]
    fn exact_and_near_small() {
        let texts = [
            "how do I reset my router password at home",
            "how do I reset my router password at home",
            "how do I reset my router password at home today",
            "completely different sentence about cooking pasta tonight",
        ];
        let sets: Vec<Vec<u64>> = texts.iter().map(|t| shingles(&normalize(t))).collect();
        let near = near_pairs(&sets, 0.7);
        assert!(near.iter().any(|p| p.0 == 0 && p.1 == 1));
        assert!(near.iter().any(|p| p.0 == 0 && p.1 == 2));
        assert!(!near.iter().any(|p| p.1 == 3 || p.0 == 3));
    }

    #[test]
    fn lsh_path_finds_planted_duplicates() {
        let mut sets: Vec<Vec<u64>> = Vec::new();
        for i in 0..1000 {
            let t = format!("record number {} talks about topic {} with unique words w{} x{} y{} z{}", i, i * 7, i * 13, i * 17, i * 19, i * 23);
            sets.push(shingles(&normalize(&t)));
        }
        sets.push(sets[10].clone());
        let near = near_pairs(&sets, 0.9);
        assert!(near.iter().any(|p| p.0 == 10 && p.1 == 1000));
    }

    #[test]
    fn cluster_union() {
        let c = clusters(5, &[vec![0, 1]], &[(1, 3, 0.9)]);
        assert_eq!(c[0], c[1]);
        assert_eq!(c[1], c[3]);
        assert_ne!(c[0], c[2]);
    }
}
