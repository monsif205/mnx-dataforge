//! Single JSON-in / JSON-out entry point used by the Android app through JNI.
//! Every call returns {"ok":true,"result":...} or {"ok":false,"error":"..."}.

use crate::json::{self, Value};
use crate::{csv, export, leak, split, stats, text, validate};
use std::collections::HashSet;

pub const VERSION: &str = "0.1.0";

pub fn call(op: &str, input: &str) -> String {
    let result = json::parse(input).map_err(|e| format!("input is not valid JSON: {}", e)).and_then(|v| dispatch(op, &v));
    match result {
        Ok(v) => Value::obj(vec![("ok", Value::Bool(true)), ("result", v)]).to_json(),
        Err(e) => Value::obj(vec![("ok", Value::Bool(false)), ("error", Value::str(e))]).to_json(),
    }
}

fn dispatch(op: &str, v: &Value) -> Result<Value, String> {
    match op {
        "version" => Ok(Value::obj(vec![("version", Value::str(VERSION))])),
        "normalize" => {
            let t = req_str(v, "text")?;
            Ok(Value::obj(vec![("text", Value::str(text::normalize(t)))]))
        }
        "parse_jsonl" => parse_jsonl(req_str(v, "text")?),
        "to_jsonl" => {
            let recs = req_arr(v, "records")?;
            let mut out = String::new();
            for r in recs {
                out.push_str(&r.to_json());
                out.push('\n');
            }
            Ok(Value::obj(vec![("text", Value::str(out))]))
        }
        "parse_json" => {
            let parsed = json::parse(req_str(v, "text")?).map_err(|e| e.to_string())?;
            let recs = match parsed {
                Value::Arr(a) => a,
                other => vec![other],
            };
            Ok(Value::obj(vec![("records", Value::Arr(recs))]))
        }
        "parse_csv" => parse_csv(v),
        "to_csv" => to_csv(v),
        "validate" => {
            let cfg = validate::Config::from_value(v.get("config").unwrap_or(&Value::Null));
            let recs = id_data(v)?;
            Ok(validate::run(&recs, &cfg))
        }
        "split" => run_split(v),
        "leakage" => run_leak(v),
        "stats" => {
            let texts: Vec<String> = req_arr(v, "texts")?.iter().map(|t| t.as_str().unwrap_or("").to_string()).collect();
            Ok(stats::run(&texts))
        }
        "record_info" => {
            let recs = id_data(v)?;
            let items: Vec<Value> = recs
                .iter()
                .map(|(id, d)| {
                    let t = text::record_text(d);
                    Value::obj(vec![
                        ("id", Value::str(id.clone())),
                        ("text", Value::str(t.clone())),
                        ("norm", Value::str(text::normalize(&t))),
                        ("script", Value::str(text::script_of(&t))),
                        ("chars", Value::int(t.chars().count() as i64)),
                        ("tokens", Value::int(text::estimate_tokens(&t) as i64)),
                    ])
                })
                .collect();
            Ok(Value::obj(vec![("items", Value::Arr(items))]))
        }
        "pretty" => crate::tools::pretty(v),
        "preview" => crate::tools::preview(v),
        "enter" => crate::tools::enter(v),
        "release_info" => crate::tools::release_info(v),
        "script" => crate::script::build(v),
        "to_script" => crate::script::render(v),
        "tokens" => crate::script::tokens(v),
        "dedup" => run_dedup(v),
        "export" => run_export(v),
        _ => Err(format!("unknown operation '{}'", op)),
    }
}

fn req_str<'a>(v: &'a Value, k: &str) -> Result<&'a str, String> {
    v.get(k).and_then(|x| x.as_str()).ok_or_else(|| format!("missing string field '{}'", k))
}
fn req_arr<'a>(v: &'a Value, k: &str) -> Result<&'a Vec<Value>, String> {
    v.get(k).and_then(|x| x.as_arr()).ok_or_else(|| format!("missing array field '{}'", k))
}

fn id_data(v: &Value) -> Result<Vec<(String, Value)>, String> {
    let mut out = Vec::new();
    for (i, r) in req_arr(v, "records")?.iter().enumerate() {
        let id = r.get("id").and_then(|x| x.as_str()).map(|s| s.to_string()).unwrap_or_else(|| format!("#{}", i));
        out.push((id, r.get("data").cloned().unwrap_or(Value::Null)));
    }
    Ok(out)
}

fn parse_jsonl(t: &str) -> Result<Value, String> {
    let t = t.strip_prefix('\u{feff}').unwrap_or(t);
    let mut recs = Vec::new();
    let mut errs = Vec::new();
    let mut blank = 0i64;
    for (i, line) in t.lines().enumerate() {
        if line.trim().is_empty() {
            blank += 1;
            continue;
        }
        match json::parse(line) {
            Ok(v) => recs.push(v),
            Err(e) => errs.push(Value::obj(vec![("line", Value::int(i as i64 + 1)), ("message", Value::str(e.to_string()))])),
        }
    }
    Ok(Value::obj(vec![("records", Value::Arr(recs)), ("errors", Value::Arr(errs)), ("blank_lines", Value::int(blank))]))
}

fn parse_csv(v: &Value) -> Result<Value, String> {
    let delim = v.get("delimiter").and_then(|d| d.as_str()).and_then(|d| d.chars().next()).unwrap_or(',');
    let rows = csv::parse(req_str(v, "text")?, delim)?;
    if rows.is_empty() {
        return Ok(Value::obj(vec![("records", Value::Arr(vec![])), ("columns", Value::Arr(vec![]))]));
    }
    let header = &rows[0];
    let mut recs = Vec::new();
    let mut errs = Vec::new();
    for (i, row) in rows[1..].iter().enumerate() {
        if row.len() != header.len() {
            errs.push(Value::obj(vec![
                ("line", Value::int(i as i64 + 2)),
                ("message", Value::str(format!("expected {} columns, found {}", header.len(), row.len()))),
            ]));
            continue;
        }
        recs.push(Value::Obj(header.iter().cloned().zip(row.iter().map(|c| Value::str(c.clone()))).collect()));
    }
    Ok(Value::obj(vec![
        ("records", Value::Arr(recs)),
        ("columns", Value::Arr(header.iter().map(|h| Value::str(h.clone())).collect())),
        ("errors", Value::Arr(errs)),
    ]))
}

fn to_csv(v: &Value) -> Result<Value, String> {
    let recs = req_arr(v, "records")?;
    let mut cols: Vec<String> = v.get("columns").and_then(|c| c.as_arr()).map(|a| a.iter().filter_map(|s| s.as_str().map(|t| t.to_string())).collect()).unwrap_or_default();
    if cols.is_empty() {
        for r in recs {
            if let Some(o) = r.as_obj() {
                for (k, _) in o {
                    if !cols.contains(k) {
                        cols.push(k.clone());
                    }
                }
            }
        }
    }
    let mut out = String::new();
    out.push_str(&csv::write_row(&cols, ','));
    out.push_str("\r\n");
    for r in recs {
        let row: Vec<String> = cols
            .iter()
            .map(|c| match r.get(c) {
                None | Some(Value::Null) => String::new(),
                Some(Value::Str(s)) => s.clone(),
                Some(other) => other.to_json(),
            })
            .collect();
        out.push_str(&csv::write_row(&row, ','));
        out.push_str("\r\n");
    }
    Ok(Value::obj(vec![("text", Value::str(out)), ("columns", Value::Arr(cols.into_iter().map(Value::str).collect()))]))
}

fn run_split(v: &Value) -> Result<Value, String> {
    let mut items = Vec::new();
    for (i, r) in req_arr(v, "records")?.iter().enumerate() {
        items.push(split::Item {
            id: r.get("id").and_then(|x| x.as_str()).map(|s| s.to_string()).unwrap_or_else(|| format!("#{}", i)),
            label: r.get("label").and_then(|x| x.as_str()).map(|s| s.to_string()),
            group: r.get("group").and_then(|x| x.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string()),
            text: r.get("text").and_then(|x| x.as_str()).map(|s| s.to_string()),
        });
    }
    let ratios: Vec<(String, f64)> = v
        .get("ratios")
        .and_then(|r| r.as_obj())
        .ok_or("missing object field 'ratios'")?
        .iter()
        .map(|(k, x)| (k.clone(), x.as_f64().unwrap_or(0.0)))
        .collect();
    let seed = v.get("seed").and_then(|s| s.as_f64()).unwrap_or(42.0) as u64;
    let dup = v.get("dup_aware").and_then(|b| b.as_bool()).unwrap_or(false);
    let thr = v.get("near_threshold").and_then(|b| b.as_f64()).unwrap_or(0.85);
    split::run(&items, &ratios, seed, dup, thr)
}

fn run_leak(v: &Value) -> Result<Value, String> {
    let recs: Vec<leak::Rec> = req_arr(v, "records")?
        .iter()
        .enumerate()
        .map(|(i, r)| leak::Rec {
            id: r.get("id").and_then(|x| x.as_str()).map(|s| s.to_string()).unwrap_or_else(|| format!("#{}", i)),
            split: r.get("split").and_then(|x| x.as_str()).unwrap_or("unassigned").to_string(),
            text: r.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            source: r.get("source").and_then(|x| x.as_str()).map(|s| s.to_string()),
        })
        .collect();
    let eval: Vec<String> = v.get("eval_splits").and_then(|a| a.as_arr()).map(|a| a.iter().filter_map(|s| s.as_str().map(|t| t.to_string())).collect()).unwrap_or_else(|| vec!["validation".into(), "test".into()]);
    let thr = v.get("threshold").and_then(|t| t.as_f64()).unwrap_or(0.85);
    let intended: HashSet<String> = v.get("intended").and_then(|a| a.as_arr()).map(|a| a.iter().filter_map(|s| s.as_str().map(|t| t.to_string())).collect()).unwrap_or_default();
    Ok(leak::run(&recs, &eval, thr, &intended))
}

fn run_dedup(v: &Value) -> Result<Value, String> {
    let items = req_arr(v, "items")?;
    let thr = v.get("threshold").and_then(|t| t.as_f64()).unwrap_or(0.85);
    let ids: Vec<String> = items.iter().enumerate().map(|(i, r)| r.get("id").and_then(|x| x.as_str()).map(|s| s.to_string()).unwrap_or_else(|| format!("#{}", i))).collect();
    let norms: Vec<String> = items.iter().map(|r| text::normalize(r.get("text").and_then(|x| x.as_str()).unwrap_or(""))).collect();
    let mut idx: Vec<usize> = Vec::new();
    let mut hashes: Vec<u64> = Vec::new();
    let mut sets: Vec<Vec<u64>> = Vec::new();
    for (i, n) in norms.iter().enumerate() {
        if n.is_empty() {
            continue;
        }
        idx.push(i);
        hashes.push(text::fnv1a(n.as_bytes()));
        sets.push(if thr > 0.0 { text::shingles(n) } else { Vec::new() });
    }
    let groups = crate::dedup::exact_groups(&hashes);
    let mut exact_out = Vec::new();
    let mut exact_pairs = HashSet::new();
    for g in &groups {
        exact_out.push(Value::Arr(g.iter().map(|m| Value::str(ids[idx[*m]].clone())).collect()));
        for m in &g[1..] {
            exact_pairs.insert((g[0].min(*m), g[0].max(*m)));
        }
    }
    let mut near_out = Vec::new();
    if thr > 0.0 {
        for (a, b, j) in crate::dedup::near_pairs(&sets, thr) {
            if exact_pairs.contains(&(a, b)) || hashes[a] == hashes[b] {
                continue;
            }
            near_out.push(Value::obj(vec![("a", Value::str(ids[idx[a]].clone())), ("b", Value::str(ids[idx[b]].clone())), ("similarity", Value::float(j))]));
        }
    }
    Ok(Value::obj(vec![("exact_groups", Value::Arr(exact_out)), ("near_pairs", Value::Arr(near_out)), ("scope", Value::int(items.len() as i64))]))
}

fn run_export(v: &Value) -> Result<Value, String> {
    let prof = export::Profile::from_value(v.get("profile").unwrap_or(&Value::Null));
    let mut lines = Vec::new();
    let mut errors = Vec::new();
    for (i, r) in req_arr(v, "records")?.iter().enumerate() {
        let id = r.get("id").and_then(|x| x.as_str()).map(|s| s.to_string()).unwrap_or_else(|| format!("#{}", i));
        let data = r.get("data").unwrap_or(&Value::Null);
        match export::convert(data, r.get("meta"), &prof) {
            Ok(out) => lines.push(Value::str(out.to_json())),
            Err(e) => errors.push(Value::obj(vec![("id", Value::str(id)), ("message", Value::str(e))])),
        }
    }
    Ok(Value::obj(vec![("lines", Value::Arr(lines)), ("errors", Value::Arr(errors))]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(op: &str, input: &str) -> Value {
        let out = json::parse(&call(op, input)).unwrap();
        assert_eq!(out.get("ok").and_then(|b| b.as_bool()), Some(true), "{}", out.to_json());
        out.get("result").unwrap().clone()
    }

    #[test]
    fn jsonl_roundtrip_with_errors_and_arabic() {
        let text = "{\"q\":\"سؤال\"}\n\n{bad}\n{\"q\":\"two\"}\n";
        let input = Value::obj(vec![("text", Value::str(text))]).to_json();
        let r = ok("parse_jsonl", &input);
        assert_eq!(r.get("records").unwrap().as_arr().unwrap().len(), 2);
        let errs = r.get("errors").unwrap().as_arr().unwrap();
        assert_eq!(errs[0].get("line").unwrap().as_f64().unwrap(), 3.0);
        let back = ok("to_jsonl", &Value::obj(vec![("records", r.get("records").unwrap().clone())]).to_json());
        assert_eq!(back.get("text").unwrap().as_str().unwrap(), "{\"q\":\"سؤال\"}\n{\"q\":\"two\"}\n");
    }

    #[test]
    fn csv_roundtrip() {
        let csv_in = "id,text\r\n1,\"مرحبا, \"\"عالم\"\"\"\r\n2,plain\r\n";
        let parsed = ok("parse_csv", &Value::obj(vec![("text", Value::str(csv_in))]).to_json());
        let back = ok("to_csv", &Value::obj(vec![("records", parsed.get("records").unwrap().clone())]).to_json());
        assert_eq!(back.get("text").unwrap().as_str().unwrap(), csv_in);
    }

    #[test]
    fn errors_are_reported_not_panicked() {
        let out = json::parse(&call("validate", "not json")).unwrap();
        assert_eq!(out.get("ok").unwrap().as_bool(), Some(false));
        let out = json::parse(&call("nope", "{}")).unwrap();
        assert_eq!(out.get("ok").unwrap().as_bool(), Some(false));
    }

    #[test]
    fn dedup_op() {
        let input = r#"{"items":[{"id":"a","text":"Hello World again"},{"id":"b","text":"hello, world again!"},{"id":"c","text":"something else entirely here"}],"threshold":0.8}"#;
        let r = ok("dedup", input);
        assert_eq!(r.get("exact_groups").unwrap().as_arr().unwrap().len(), 1);
    }

    #[test]
    fn validate_via_api() {
        let input = r#"{"config":{"kind":"qa"},"records":[{"id":"a","data":{"question":"q","answer":""}}]}"#;
        let r = ok("validate", input);
        assert_eq!(r.get("errors").unwrap().as_f64().unwrap(), 1.0);
    }
}
