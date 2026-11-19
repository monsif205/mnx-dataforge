//! XLSX import: reads the first worksheet (shared strings, inline strings, numbers, booleans) and returns it as CSV text,
//! so the normal CSV reader (header row, quoting, field mapping) does the rest. Dates stay as Excel serial numbers.

use crate::archive;
use crate::docx::decode_entities;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

fn part(path: &Path, entries: &[archive::ZipEntry], name: &str, cap: usize) -> Result<Option<String>, String> {
    let e = match entries.iter().find(|e| e.name == name) {
        Some(e) => e,
        None => return Ok(None),
    };
    let mut rd = archive::zip_open(path, e, 64 << 20, Arc::new(AtomicU64::new(0))).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    (&mut rd).take(cap as u64 + 1).read_to_end(&mut raw).map_err(|e| format!("damaged part {}: {}", name, e))?;
    if raw.len() > cap {
        return Err(format!("the sheet is larger than the {} MiB limit", cap >> 20));
    }
    Ok(Some(String::from_utf8_lossy(&raw).into_owned()))
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let k = format!("{}=\"", name);
    let i = tag.find(&k)? + k.len();
    let j = tag[i..].find('"')?;
    Some(decode_entities(&tag[i..i + j]))
}

/// Text inside every <t>…</t> of a fragment (rich-text runs are joined; phonetic runs are skipped).
fn texts(x: &str) -> String {
    let x = match x.find("<rPh") {
        Some(_) => {
            let mut s = String::new();
            let mut rest = x;
            while let Some(i) = rest.find("<rPh") {
                s.push_str(&rest[..i]);
                rest = match rest[i..].find("</rPh>") {
                    Some(j) => &rest[i + j + 6..],
                    None => "",
                };
            }
            s.push_str(rest);
            std::borrow::Cow::Owned(s)
        }
        None => std::borrow::Cow::Borrowed(x),
    };
    let mut out = String::new();
    let mut rest: &str = &x;
    while let Some(i) = rest.find("<t") {
        let after = &rest[i + 2..];
        let c = after.chars().next().unwrap_or(' ');
        if c != '>' && c != ' ' && c != '/' {
            rest = after;
            continue;
        }
        let gt = match after.find('>') {
            Some(g) => g,
            None => break,
        };
        if after[..gt].ends_with('/') {
            rest = &after[gt + 1..];
            continue;
        }
        let body = &after[gt + 1..];
        let end = body.find("</t>").unwrap_or(body.len());
        out.push_str(&decode_entities(&body[..end]));
        rest = &body[end..];
    }
    out
}

fn col_index(r: &str) -> usize {
    let mut n = 0usize;
    for c in r.chars().take_while(|c| c.is_ascii_alphabetic()) {
        n = n * 26 + (c.to_ascii_uppercase() as u8 - b'A' + 1) as usize;
    }
    n.saturating_sub(1)
}

fn csv_cell(s: &str, out: &mut String) {
    if s.contains(|c| c == ',' || c == '"' || c == '\n' || c == '\r') {
        out.push('"');
        out.push_str(&s.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(s);
    }
}

pub fn sheet_to_csv(sheet: &str, shared: &[String]) -> String {
    let mut out = String::new();
    let mut rest = sheet;
    let mut next_row = 0usize;
    while let Some(i) = rest.find("<row") {
        let after = &rest[i + 4..];
        if !after.starts_with(|c: char| c == ' ' || c == '>' || c == '/') {
            rest = after;
            continue;
        }
        let gt = match after.find('>') {
            Some(g) => g,
            None => break,
        };
        let row_tag = &after[..gt];
        let self_closed = row_tag.ends_with('/');
        let (body, adv) = if self_closed {
            ("", gt + 1)
        } else {
            let e = after.find("</row>").unwrap_or(after.len());
            (&after[gt + 1..e.max(gt + 1)], e)
        };
        rest = &after[adv.min(after.len())..];
        let rn = attr(row_tag, "r").and_then(|r| r.parse::<usize>().ok()).map(|r| r.saturating_sub(1)).unwrap_or(next_row);
        while next_row < rn.min(next_row + 1_000_000) {
            out.push('\n');
            next_row += 1;
        }
        next_row = rn + 1;
        let mut cells: Vec<(usize, String)> = Vec::new();
        let mut cur = body;
        let mut pos = 0usize;
        while let Some(c) = cur.find("<c") {
            let a = &cur[c + 2..];
            if !a.starts_with(|ch: char| ch == ' ' || ch == '>' || ch == '/') {
                cur = a;
                continue;
            }
            let g = match a.find('>') {
                Some(g) => g,
                None => break,
            };
            let ctag = &a[..g];
            let (inner, adv2) = if ctag.ends_with('/') {
                ("", g + 1)
            } else {
                let e = a.find("</c>").unwrap_or(a.len());
                (&a[g + 1..e.max(g + 1)], e)
            };
            cur = &a[adv2.min(a.len())..];
            let col = attr(ctag, "r").map(|r| col_index(&r)).unwrap_or(pos);
            pos = col + 1;
            let t = attr(ctag, "t").unwrap_or_default();
            let v = inner.find("<v>").and_then(|s| inner[s + 3..].find("</v>").map(|e| decode_entities(&inner[s + 3..s + 3 + e])));
            let val = match t.as_str() {
                "s" => v.and_then(|x| x.trim().parse::<usize>().ok()).and_then(|k| shared.get(k).cloned()).unwrap_or_default(),
                "inlineStr" => texts(inner),
                "b" => (if v.as_deref() == Some("1") { "TRUE" } else { "FALSE" }).to_string(),
                _ => v.unwrap_or_default(),
            };
            if col < 16_384 {
                cells.push((col, val));
            }
        }
        let width = cells.iter().map(|(c, _)| c + 1).max().unwrap_or(0);
        let mut line: Vec<&str> = vec![""; width];
        for (c, v) in &cells {
            line[*c] = v;
        }
        for (k, v) in line.iter().enumerate() {
            if k > 0 {
                out.push(',');
            }
            csv_cell(v, &mut out);
        }
        out.push('\n');
    }
    out
}

pub fn extract(path: &Path, cap: usize) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("cannot open the file: {}", e))?;
    let entries = archive::zip_entries(&mut f, 100_000).map_err(|e| format!("not a valid XLSX package: {}", e))?;
    if !entries.iter().any(|e| e.name == "xl/workbook.xml") {
        return Err("not an Excel workbook: xl/workbook.xml is missing".into());
    }
    let mut shared = Vec::new();
    if let Some(ss) = part(path, &entries, "xl/sharedStrings.xml", cap)? {
        let mut rest = ss.as_str();
        while let Some(i) = rest.find("<si") {
            let a = &rest[i + 3..];
            if !a.starts_with(|c: char| c == '>' || c == ' ' || c == '/') {
                rest = a;
                continue;
            }
            let e = a.find("</si>").unwrap_or(a.len());
            shared.push(texts(&a[..e]));
            rest = &a[e.min(a.len())..];
        }
    }
    // first sheet in workbook order, else the lowest-numbered worksheet part
    let mut target: Option<String> = None;
    if let (Some(wb), Some(rels)) = (part(path, &entries, "xl/workbook.xml", cap)?, part(path, &entries, "xl/_rels/workbook.xml.rels", cap)?) {
        if let Some(s) = wb.find("<sheet ") {
            let rid = attr(&wb[s..], "r:id");
            if let Some(rid) = rid {
                let mut r = rels.as_str();
                while let Some(i) = r.find("<Relationship ") {
                    let tag = &r[i..r[i..].find('>').map(|g| i + g).unwrap_or(r.len())];
                    if attr(tag, "Id").as_deref() == Some(rid.as_str()) {
                        if let Some(t) = attr(tag, "Target") {
                            target = Some(if let Some(a) = t.strip_prefix('/') { a.to_string() } else { format!("xl/{}", t) });
                        }
                        break;
                    }
                    r = &r[i + 14..];
                }
            }
        }
    }
    let name = target.filter(|t| entries.iter().any(|e| &e.name == t)).or_else(|| {
        let mut n: Vec<&str> = entries.iter().map(|e| e.name.as_str()).filter(|n| n.starts_with("xl/worksheets/") && n.ends_with(".xml")).collect();
        n.sort();
        n.first().map(|s| s.to_string())
    });
    let name = name.ok_or("the workbook has no worksheets")?;
    let sheet = part(path, &entries, &name, cap)?.ok_or("worksheet part missing")?;
    Ok(sheet_to_csv(&sheet, &shared))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cells_to_csv() {
        let sh = r#"<sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="C1" t="inlineStr"><is><t>x,y</t></is></c></row><row r="3"><c r="B3"><v>4.5</v></c><c r="C3" t="b"><v>1</v></c></row></sheetData>"#;
        let t = sheet_to_csv(sh, &["name".to_string()]);
        assert_eq!(t, "name,,\"x,y\"\n\n,4.5,TRUE\n");
    }
    #[test]
    fn real_workbook() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/sample.xlsx");
        let t = extract(&p, 1 << 26).unwrap();
        assert!(t.starts_with("name,score\n") && t.contains("Ali,10") && t.contains("مرحبا"), "{}", t);
    }
}
