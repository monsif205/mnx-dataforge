//! DOCX text extraction: reads word/document.xml from the ZIP package and returns Markdown-style text
//! (headings as "#", list items as "-", tables as "| a | b |" rows, paragraphs separated by a blank line).
//! The XML is scanned, not loaded as a tree, and the part is read under a size cap.

use crate::archive;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        if let Some(j) = rest.find(';').filter(|&j| j <= 10) {
            let ent = &rest[1..j];
            let ch = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
                e if e.starts_with('#') => e[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            if let Some(c) = ch {
                out.push(c);
                rest = &rest[j + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &rest[1..];
    }
    out.push_str(rest);
    out
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{}=\"", name);
    let i = tag.find(&key)? + key.len();
    let j = tag[i..].find('"')?;
    Some(&tag[i..i + j])
}

fn heading_level(style: &str) -> Option<usize> {
    let l = style.to_lowercase().replace(' ', "");
    if l == "title" {
        return Some(1);
    }
    let d = l.strip_prefix("heading")?;
    d.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse::<usize>().ok().map(|n| n.clamp(1, 6))
}

/// Converts the XML of word/document.xml to Markdown-style text.
pub fn xml_to_text(xml: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut para = String::new();
    let mut style: Option<String> = None;
    let mut listed = false;
    let mut in_p = false;
    let mut in_t = false;
    let mut tbl_depth = 0usize;
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let b = xml.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'<' {
            let end = match xml[i..].find('>') {
                Some(e) => i + e,
                None => break,
            };
            let tag = &xml[i + 1..end];
            let closing = tag.starts_with('/');
            let nm = tag.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");
            match (closing, nm) {
                (false, "w:p") => {
                    in_p = true;
                    para.clear();
                    style = None;
                    listed = false;
                }
                (false, "w:pStyle") => style = attr(tag, "w:val").map(|s| s.to_string()),
                (false, "w:numPr") => listed = true,
                (false, "w:t") => in_t = !tag.ends_with('/'),
                (true, "w:t") => in_t = false,
                (false, "w:tab") if in_p => para.push(' '),
                (false, "w:br") | (false, "w:cr") if in_p => para.push('\n'),
                (false, "w:tbl") => tbl_depth += 1,
                (true, "w:tbl") => {
                    tbl_depth = tbl_depth.saturating_sub(1);
                    if tbl_depth == 0 {
                        for r in rows.drain(..) {
                            out.push(format!("| {} |", r.join(" | ")));
                        }
                    }
                }
                (false, "w:tr") => row.clear(),
                (true, "w:tr") => {
                    if tbl_depth == 1 {
                        rows.push(std::mem::take(&mut row));
                    }
                }
                (false, "w:tc") => cell.clear(),
                (true, "w:tc") => {
                    if tbl_depth >= 1 {
                        row.push(cell.trim().replace('\n', " "));
                    }
                }
                (true, "w:p") => {
                    in_p = false;
                    let t = para.trim_end().to_string();
                    if tbl_depth > 0 {
                        if !t.is_empty() {
                            if !cell.is_empty() {
                                cell.push(' ');
                            }
                            cell.push_str(&t);
                        }
                    } else if !t.trim().is_empty() {
                        let line = match style.as_deref().and_then(heading_level) {
                            Some(n) => format!("{} {}", "#".repeat(n), t.trim()),
                            None if listed => format!("- {}", t.trim()),
                            None => t,
                        };
                        out.push(line);
                    }
                }
                _ => {}
            }
            i = end + 1;
        } else {
            let next = xml[i..].find('<').map(|e| i + e).unwrap_or(b.len());
            if in_t && in_p {
                para.push_str(&decode_entities(&xml[i..next]));
            }
            i = next;
        }
    }
    // paragraphs are blank-line separated; consecutive list items and table rows stay together
    let mut text = String::new();
    for (k, line) in out.iter().enumerate() {
        if k > 0 {
            let both_list = line.starts_with("- ") && out[k - 1].starts_with("- ");
            let both_row = line.starts_with('|') && out[k - 1].starts_with('|');
            text.push_str(if both_list || both_row { "\n" } else { "\n\n" });
        }
        text.push_str(line);
    }
    text
}

/// Reads a .docx file from disk. `cap` limits the size of the XML part (decompressed).
pub fn extract(path: &Path, cap: usize) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("cannot open the file: {}", e))?;
    let entries = archive::zip_entries(&mut f, 100_000).map_err(|e| format!("not a valid DOCX package: {}", e))?;
    let e = entries.iter().find(|e| e.name == "word/document.xml").ok_or("not a Word document: word/document.xml is missing")?;
    let mut rd = archive::zip_open(path, e, 64 << 20, Arc::new(AtomicU64::new(0))).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    (&mut rd).take(cap as u64 + 1).read_to_end(&mut raw).map_err(|e| format!("damaged document part: {}", e))?;
    if raw.len() > cap {
        return Err(format!("the document text is larger than the {} MiB limit", cap >> 20));
    }
    Ok(xml_to_text(&String::from_utf8_lossy(&raw)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_lists_tables_entities() {
        let xml = r#"<w:document><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Title &amp; more</w:t></w:r></w:p>
<w:p><w:r><w:t xml:space="preserve">Hello </w:t></w:r><w:r><w:t>world</w:t></w:r><w:r><w:br/><w:t>line2</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/></w:numPr></w:pPr><w:r><w:t>item one</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/></w:numPr></w:pPr><w:r><w:t>item two</w:t></w:r></w:p>
<w:tbl><w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
<w:p><w:r><w:t>مرحبا &#1575;</w:t></w:r></w:p>
</w:body></w:document>"#;
        let t = xml_to_text(xml);
        assert_eq!(t, "# Title & more\n\nHello world\nline2\n\n- item one\n- item two\n\n| a | b |\n\nمرحبا ا");
    }

    #[test]
    fn real_file() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/sample.docx");
        let t = extract(&p, 1 << 20).unwrap();
        assert!(t.starts_with("# ") && t.contains("- ") && t.contains('|'), "{}", t);
        assert!(extract(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/pq_plain.parquet"), 1 << 20).is_err());
    }
}
