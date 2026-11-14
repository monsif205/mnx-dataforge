//! Host benchmark of the engine (NOT a device measurement). Run: cargo run --release --example bench -p dataforge-core
use dataforge_core::api;
use dataforge_core::json::Value;
use std::time::Instant;

fn main() {
    let n: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(50_000);
    let mut recs = Vec::with_capacity(n);
    for i in 0..n {
        let q = format!("سؤال رقم {} حول الموضوع {} وتفاصيل إضافية {} {}", i, i % 97, i * 31 % 1013, i * 17 % 509);
        let a = format!("الجواب رقم {} مع شرح مفصل يتعلق بالموضوع {} و{}", i, i % 97, i * 7 % 211);
        let data = Value::obj(vec![("messages", Value::Arr(vec![
            Value::obj(vec![("role", Value::str("user")), ("content", Value::str(q))]),
            Value::obj(vec![("role", Value::str("assistant")), ("content", Value::str(a))]),
        ]))]);
        recs.push(Value::obj(vec![("id", Value::str(format!("r{}", i))), ("data", data)]));
    }
    let input = Value::obj(vec![
        ("config", Value::obj(vec![("kind", Value::str("conversation")), ("near_threshold", Value::float(0.85))])),
        ("records", Value::Arr(recs.clone())),
    ]).to_json();
    println!("records: {}  input bytes: {}", n, input.len());
    let t = Instant::now();
    let out = api::call("validate", &input);
    println!("validate (incl. exact + near dedup): {} ms, output bytes {}", t.elapsed().as_millis(), out.len());

    let items: Vec<Value> = recs.iter().map(|r| Value::obj(vec![
        ("id", r.get("id").unwrap().clone()),
        ("text", Value::str(dataforge_core::text::record_text(r.get("data").unwrap()))),
    ])).collect();
    let dinput = Value::obj(vec![("items", Value::Arr(items)), ("threshold", Value::float(0.85))]).to_json();
    let t = Instant::now();
    let out = api::call("dedup", &dinput);
    println!("dedup only: {} ms, output bytes {}", t.elapsed().as_millis(), out.len());

    let sitems: Vec<Value> = recs.iter().map(|r| Value::obj(vec![("id", r.get("id").unwrap().clone()), ("label", Value::str(format!("c{}", r.get("id").unwrap().as_str().unwrap().len() % 5)))])).collect();
    let sinput = Value::obj(vec![("records", Value::Arr(sitems)), ("ratios", Value::obj(vec![("train", Value::float(0.8)), ("validation", Value::float(0.1)), ("test", Value::float(0.1))])), ("seed", Value::int(42))]).to_json();
    let t = Instant::now();
    let _ = api::call("split", &sinput);
    println!("split (stratified): {} ms", t.elapsed().as_millis());
}
