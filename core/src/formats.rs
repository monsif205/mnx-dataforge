//! More document formats, all read as plain text: EPUB, OpenDocument (odt/odp/ods), PPTX, RTF, Jupyter notebooks, SRT/VTT subtitles.
//! ZIP-based parts are read with a size cap. Text only: images, styles, comments and tracked changes are ignored.

use crate::archive;
use crate::docx::decode_entities;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

fn read_part(path: &Path, entries: &[archive::ZipEntry], name: &str, cap: usize) -> Result<Option<String>, String> {
    let e = match entries.iter().find(|e| e.name == name) {
        Some(e) => e,
        None => return Ok(None),
    };
    let mut rd = archive::zip_open(path, e, 64 << 20, Arc::new(AtomicU64::new(0))).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    (&mut rd).take(cap as u64 + 1).read_to_end(&mut raw).map_err(|e| format!("damaged part {}: {}", name, e))?;
    if raw.len() > cap {
        return Err(format!("{} is larger than the {} MiB limit", name, cap >> 20));
    }
    Ok(Some(String::from_utf8_lossy(&raw).into_owned()))
}

fn entries_of(path: &Path) -> Result<Vec<archive::ZipEntry>, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("cannot open the file: {}", e))?;
    archive::zip_entries(&mut f, 200_000).map_err(|e| format!("not a valid package: {}", e))
}

fn attr(tag: &str, name: &str) -> Option<String> {
    for q in ['"', '\''] {
        let k = format!("{}={}", name, q);
        if let Some(i) = tag.find(&k) {
            let s = i + k.len();
            if let Some(j) = tag[s..].find(q) {
                return Some(decode_entities(&tag[s..s + j]));
            }
        }
    }
    None
}

/// Generic XML to text. `blocks` end a paragraph, `breaks` insert a newline, `cells` end a table cell, `text_tag` (if any) restricts text to that element.
fn xml_text(xml: &str, text_tag: Option<&str>, blocks: &[&str], breaks: &[&str], cells: &[&str]) -> String {
    let mut out = String::new();
    let mut i = 0;
    let b = xml.as_bytes();
    let mut in_text = text_tag.is_none();
    let mut in_cell = false;
    while i < b.len() {
        if b[i] == b'<' {
            let end = match xml[i..].find('>') {
                Some(e) => i + e,
                None => break,
            };
            let tag = &xml[i + 1..end];
            let closing = tag.starts_with('/');
            let name = tag.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");
            let selfc = tag.ends_with('/');
            if let Some(tt) = text_tag {
                if name == tt {
                    in_text = !closing && !selfc;
                }
            }
            if cells.contains(&name) {
                in_cell = !closing && !selfc;
            }
            if closing && blocks.contains(&name) {
                if !in_cell {
                    out.push('\n');
                }
            } else if (!closing && breaks.contains(&name)) || (closing && breaks.contains(&name) && !selfc && false) {
                out.push('\n');
            } else if closing && cells.contains(&name) {
                out.push_str(" | ");
            }
            if name == "text:tab" {
                out.push('\t');
            }
            if name == "text:s" {
                out.push(' ');
            }
            i = end + 1;
        } else {
            let end = xml[i..].find('<').map(|e| i + e).unwrap_or(b.len());
            if in_text {
                out.push_str(&decode_entities(&xml[i..end]));
            }
            i = end;
        }
    }
    tidy(&out)
}

fn tidy(s: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for l in s.split('\n') {
        let t = l.trim_end().trim_end_matches('|').trim_end().to_string();
        lines.push(t);
    }
    let mut out = String::new();
    let mut blank = 0;
    for l in lines {
        if l.trim().is_empty() {
            blank += 1;
            if blank <= 1 && !out.is_empty() {
                out.push('\n');
            }
        } else {
            if blank == 0 && !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            blank = 0;
            out.push_str(&l);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

pub fn epub(path: &Path, cap: usize) -> Result<String, String> {
    let entries = entries_of(path)?;
    let container = read_part(path, &entries, "META-INF/container.xml", cap)?.ok_or("not an EPUB: META-INF/container.xml is missing")?;
    let opf_path = container.find("full-path=").and_then(|_| attr(&container[container.find("<rootfile").unwrap_or(0)..], "full-path")).ok_or("EPUB has no package document")?;
    let opf = read_part(path, &entries, &opf_path, cap)?.ok_or("EPUB package document is missing")?;
    let base = opf_path.rsplit_once('/').map(|(d, _)| format!("{}/", d)).unwrap_or_default();
    let mut manifest: Vec<(String, String)> = Vec::new();
    let mut r = opf.as_str();
    while let Some(i) = r.find("<item ") {
        let end = r[i..].find('>').map(|e| i + e).unwrap_or(r.len());
        let tag = &r[i..end];
        if let (Some(id), Some(href)) = (attr(tag, "id"), attr(tag, "href")) {
            manifest.push((id, href));
        }
        r = &r[end..];
    }
    let mut order: Vec<String> = Vec::new();
    let mut r = opf.as_str();
    while let Some(i) = r.find("<itemref ") {
        let end = r[i..].find('>').map(|e| i + e).unwrap_or(r.len());
        if let Some(id) = attr(&r[i..end], "idref") {
            if let Some((_, h)) = manifest.iter().find(|(m, _)| *m == id) {
                order.push(format!("{}{}", base, h));
            }
        }
        r = &r[end..];
    }
    if order.is_empty() {
        order = entries.iter().map(|e| e.name.clone()).filter(|n| !n.starts_with("META-INF") && (n.ends_with(".xhtml") || n.ends_with(".html") || n.ends_with(".htm"))).collect();
        order.sort();
    }
    let mut parts = Vec::new();
    let mut total = 0usize;
    for name in order {
        let name = name.split('#').next().unwrap_or("").to_string();
        if let Some(h) = read_part(path, &entries, &name, cap)? {
            let d = crate::html::extract(&h, &crate::html::Opts { markdown: true, ..Default::default() });
            if !d.text.trim().is_empty() {
                total += d.text.len();
                parts.push(d.text);
                if total > cap {
                    return Err(format!("the book text is larger than the {} MiB limit", cap >> 20));
                }
            }
        }
    }
    if parts.is_empty() {
        return Err("no readable text found in the EPUB".into());
    }
    Ok(parts.join("\n\n"))
}

pub fn opendocument(path: &Path, cap: usize) -> Result<String, String> {
    let entries = entries_of(path)?;
    let c = read_part(path, &entries, "content.xml", cap)?.ok_or("not an OpenDocument file: content.xml is missing")?;
    let body = c.find("<office:body").map(|i| &c[i..]).unwrap_or(&c);
    let t = xml_text(body, None, &["text:p", "text:h", "table:table-row"], &["text:line-break"], &["table:table-cell"]);
    if t.is_empty() {
        return Err("no text found in the document".into());
    }
    Ok(t)
}

pub fn pptx(path: &Path, cap: usize) -> Result<String, String> {
    let entries = entries_of(path)?;
    let mut slides: Vec<(usize, String)> = entries
        .iter()
        .filter_map(|e| {
            let n = e.name.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?;
            Some((n.parse::<usize>().ok()?, e.name.clone()))
        })
        .collect();
    if slides.is_empty() {
        return Err("not a PowerPoint file: no slides found".into());
    }
    slides.sort();
    let mut parts = Vec::new();
    for (n, name) in slides {
        if let Some(x) = read_part(path, &entries, &name, cap)? {
            let t = xml_text(&x, Some("a:t"), &["a:p"], &["a:br"], &[]);
            if !t.is_empty() {
                parts.push(format!("## Slide {}\n\n{}", n, t));
            }
        }
    }
    if parts.is_empty() {
        return Err("no text found on the slides".into());
    }
    Ok(parts.join("\n\n"))
}

fn cp1252(b: u8) -> char {
    match b {
        0x80 => '€', 0x91 => '‘', 0x92 => '’', 0x93 => '“', 0x94 => '”', 0x95 => '•', 0x96 => '–', 0x97 => '—', 0x85 => '…',
        _ => b as char,
    }
}

pub fn rtf_text(src: &[u8]) -> String {
    let mut out = String::new();
    let mut stack: Vec<(bool, usize)> = Vec::new(); // (skipping, unicode skip count)
    let mut skip = false;
    let mut uc = 1usize;
    let mut pending_skip = 0usize;
    let mut i = 0;
    while i < src.len() {
        let c = src[i];
        match c {
            b'{' => {
                stack.push((skip, uc));
                i += 1;
                // destinations that are not text
                if src.get(i) == Some(&b'\\') {
                    let rest = &src[i + 1..];
                    let w: String = rest.iter().take_while(|b| b.is_ascii_alphabetic() || **b == b'*').map(|&b| b as char).collect();
                    if w == "*" || ["fonttbl", "colortbl", "stylesheet", "info", "pict", "header", "footer", "footnote", "themedata", "colorschememapping", "latentstyles", "datastore", "listtable", "listoverridetable", "rsidtbl", "generator"].contains(&w.as_str()) {
                        skip = true;
                    }
                }
            }
            b'}' => {
                if let Some((s, u)) = stack.pop() {
                    skip = s;
                    uc = u;
                }
                i += 1;
            }
            b'\\' => {
                i += 1;
                let n = match src.get(i) {
                    Some(&n) => n,
                    None => break,
                };
                if n.is_ascii_alphabetic() {
                    let st = i;
                    while i < src.len() && src[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let word = String::from_utf8_lossy(&src[st..i]).to_string();
                    let ns = i;
                    if i < src.len() && (src[i] == b'-' || src[i].is_ascii_digit()) {
                        i += 1;
                        while i < src.len() && src[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                    let param: Option<i32> = std::str::from_utf8(&src[ns..i]).ok().and_then(|s| s.parse().ok());
                    if i < src.len() && src[i] == b' ' {
                        i += 1;
                    }
                    match word.as_str() {
                        "par" | "line" | "sect" | "page" if !skip => out.push('\n'),
                        "tab" if !skip => out.push('\t'),
                        "cell" if !skip => out.push_str(" | "),
                        "row" if !skip => out.push('\n'),
                        "emdash" if !skip => out.push('—'),
                        "endash" if !skip => out.push('–'),
                        "bullet" if !skip => out.push('•'),
                        "lquote" if !skip => out.push('‘'),
                        "rquote" if !skip => out.push('’'),
                        "ldblquote" if !skip => out.push('“'),
                        "rdblquote" if !skip => out.push('”'),
                        "uc" => uc = param.unwrap_or(1).max(0) as usize,
                        "u" if !skip => {
                            if let Some(p) = param {
                                let cp = if p < 0 { (p + 65536) as u32 } else { p as u32 };
                                if let Some(ch) = char::from_u32(cp) {
                                    out.push(ch);
                                }
                                pending_skip = uc;
                            }
                        }
                        _ => {}
                    }
                } else {
                    i += 1;
                    match n {
                        b'\'' => {
                            let h = std::str::from_utf8(src.get(i..i + 2).unwrap_or(b"00")).unwrap_or("00");
                            if let Ok(v) = u8::from_str_radix(h, 16) {
                                if !skip {
                                    if pending_skip > 0 {
                                        pending_skip -= 1;
                                    } else {
                                        out.push(cp1252(v));
                                    }
                                }
                            }
                            i += 2;
                        }
                        b'\\' | b'{' | b'}' if !skip => out.push(n as char),
                        b'~' if !skip => out.push('\u{a0}'),
                        b'-' | b'_' | b'*' | b':' => {}
                        _ => {}
                    }
                }
            }
            b'\r' | b'\n' => i += 1,
            _ => {
                if !skip {
                    if pending_skip > 0 {
                        pending_skip -= 1;
                    } else {
                        // keep UTF-8 sequences intact
                        let len = match c { 0xF0..=0xF7 => 4, 0xE0..=0xEF => 3, 0xC0..=0xDF => 2, _ => 1 };
                        match std::str::from_utf8(&src[i..(i + len).min(src.len())]) {
                            Ok(s) if len > 1 => out.push_str(s),
                            _ => out.push(c as char),
                        }
                        i += len;
                        continue;
                    }
                }
                i += 1;
            }
        }
    }
    tidy(&out)
}

pub fn ipynb(src: &str) -> Result<String, String> {
    let v = crate::json::parse(src).map_err(|e| format!("not a valid notebook: {}", e.msg))?;
    let cells = v.get("cells").and_then(|c| c.as_arr()).ok_or("not a Jupyter notebook: no cells")?;
    let lang = v.get_path("metadata.kernelspec.language").and_then(|l| l.as_str()).unwrap_or("python").to_string();
    let mut parts = Vec::new();
    for c in cells {
        let kind = c.get("cell_type").and_then(|k| k.as_str()).unwrap_or("");
        let text = match c.get("source") {
            Some(crate::json::Value::Str(s)) => s.clone(),
            Some(crate::json::Value::Arr(a)) => a.iter().filter_map(|x| x.as_str()).collect::<String>(),
            _ => continue,
        };
        if text.trim().is_empty() {
            continue;
        }
        parts.push(match kind {
            "code" => format!("```{}\n{}\n```", lang, text.trim_end()),
            _ => text.trim().to_string(),
        });
    }
    if parts.is_empty() {
        return Err("the notebook has no text cells".into());
    }
    Ok(parts.join("\n\n"))
}

pub fn subtitles(src: &str) -> String {
    let mut cues: Vec<String> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    fn flush(cur: &mut Vec<String>, cues: &mut Vec<String>) {
        if !cur.is_empty() {
            cues.push(cur.join(" "));
            cur.clear();
        }
    }
    for l in src.trim_start_matches('\u{feff}').lines() {
        let t = l.trim();
        if t.is_empty() {
            flush(&mut cur, &mut cues);
            continue;
        }
        if t.contains("-->") || t.starts_with("WEBVTT") || t.starts_with("NOTE") || t.starts_with("STYLE") || (cur.is_empty() && t.chars().all(|c| c.is_ascii_digit())) {
            continue;
        }
        let mut clean = String::new();
        let mut in_tag = false;
        for c in t.chars() {
            match c {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => clean.push(c),
                _ => {}
            }
        }
        let clean = decode_entities(&clean);
        if !clean.trim().is_empty() {
            cur.push(clean.trim().to_string());
        }
    }
    if !cur.is_empty() {
        cues.push(cur.join(" "));
    }
    cues.dedup();
    cues.join("\n")
}

pub fn extract(kind: &str, path: &Path, cap: usize) -> Result<String, String> {
    match kind {
        "epub" => epub(path, cap),
        "odt" => opendocument(path, cap),
        "pptx" => pptx(path, cap),
        "rtf" | "ipynb" | "srt" | "vtt" => {
            let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len() as usize;
            if len > cap {
                return Err(format!("the file is larger than the {} MiB limit", cap >> 20));
            }
            let d = std::fs::read(path).map_err(|e| e.to_string())?;
            Ok(match kind {
                "rtf" => rtf_text(&d),
                "ipynb" => return ipynb(&String::from_utf8_lossy(&d)),
                _ => subtitles(&String::from_utf8_lossy(&d)),
            })
        }
        other => Err(format!("unsupported document type {}", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rtf_basic() {
        let t = rtf_text(br"{\rtf1\ansi{\fonttbl{\f0 Arial;}}\f0\fs24 Hello {\b bold} world\par Caf\'e9 \u1575? end\par}");
        assert_eq!(t, "Hello bold world\nCafé ا end", "{}", t);
    }
    #[test]
    fn notebook() {
        let t = ipynb(r##"{"metadata":{"kernelspec":{"language":"python"}},"cells":[{"cell_type":"markdown","source":["# Title\n","text"]},{"cell_type":"code","source":"print(1)"}]}"##).unwrap();
        assert_eq!(t, "# Title\ntext\n\n```python\nprint(1)\n```");
    }
    #[test]
    fn srt_vtt() {
        let t = subtitles("1\n00:00:01,000 --> 00:00:02,000\n<i>Hello</i>\nthere\n\n2\n00:00:03,000 --> 00:00:04,000\nBye\n");
        assert_eq!(t, "Hello there\nBye");
        assert_eq!(subtitles("WEBVTT\n\n00:01.000 --> 00:02.000\nHi &amp; ok\n"), "Hi & ok");
    }
    #[test]
    fn xml_to_text_odf_and_pptx() {
        let t = xml_text(r#"<o><text:h>Title</text:h><text:p>A<text:s/>b</text:p><table:table-row><table:table-cell><text:p>x</text:p></table:table-cell><table:table-cell><text:p>y</text:p></table:table-cell></table:table-row></o>"#, None, &["text:p", "text:h", "table:table-row"], &["text:line-break"], &["table:table-cell"]);
        assert_eq!(t, "Title\nA b\nx | y", "{:?}", t);
        let p = xml_text(r#"<p><a:p><a:r><a:t>One</a:t></a:r></a:p><a:p><a:r><a:t>Two</a:t></a:r></a:p></p>"#, Some("a:t"), &["a:p"], &["a:br"], &[]);
        assert_eq!(p, "One\nTwo");
    }

    #[test]
    fn real_files() {
        let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
        let odt = extract("odt", &d.join("sample.odt"), 1 << 26).unwrap();
        assert!(odt.contains("First paragraph") && odt.contains("cell A"), "{}", odt);
        let rtf = extract("rtf", &d.join("sample.rtf"), 1 << 26).unwrap();
        assert!(rtf.contains("First paragraph") && rtf.contains("cell A"), "{}", rtf);
        let pp = extract("pptx", &d.join("s.pptx"), 1 << 26).unwrap();
        assert!(pp.contains("## Slide 1") && pp.contains("Deck title") && pp.contains("bullet one"), "{}", pp);
    }
}
