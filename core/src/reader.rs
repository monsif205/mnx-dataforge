//! Streaming record readers over files, archives and compressed streams, with handles so a caller can pull
//! bounded batches. Every reader enforces size limits, reports damaged data as errors (the rest of the file is
//! still read) and never loads a whole large file into memory.

use crate::archive::{self, bad, Counting, Kind, Limited, SharedTar, TarData, ZipEntry};
use crate::csv;
use crate::inflate::Gunzip;
use crate::json::{self, Value};
use crate::zstd::Zstd;
use std::collections::{HashMap, VecDeque};
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone)]
pub struct Limits {
    pub max_line_bytes: usize,
    pub max_file_bytes: u64, // per decompressed stream; 0 = unlimited
    pub max_window: usize,
    pub max_entries: usize,
    pub tmp_dir: PathBuf,
    pub csv_delim: Option<char>,
    pub text_mode: String, // plain | conversation | fields
    pub text_split: String, // lines | paragraphs
    pub fields: Vec<String>,
    pub extra_roles: Vec<String>,
    pub entry: Option<String>,
    pub html_markdown: bool,
    pub whole_file_cap: usize, // limit for formats that need the whole text (html, json object, labelled text)
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_line_bytes: 16 << 20,
            max_file_bytes: 0,
            max_window: 64 << 20,
            max_entries: 100_000,
            tmp_dir: std::env::temp_dir(),
            csv_delim: None,
            text_mode: "plain".into(),
            text_split: "lines".into(),
            fields: vec![],
            extra_roles: vec![],
            entry: None,
            html_markdown: true,
            whole_file_cap: 64 << 20,
        }
    }
}

#[derive(Default)]
pub struct Batch {
    pub records: Vec<Value>,
    pub sources: Vec<String>,
    pub errors: Vec<(String, String)>,
    pub errors_dropped: u64,
    pub bytes: usize,
}

impl Batch {
    pub fn push(&mut self, source: &str, v: Value) {
        self.bytes += approx_size(&v);
        self.records.push(v);
        self.sources.push(source.to_string());
    }
    pub fn err(&mut self, wh: &str, msg: impl Into<String>) {
        if self.errors.len() < 200 {
            self.errors.push((wh.to_string(), msg.into()));
        } else {
            self.errors_dropped += 1;
        }
    }
    fn full(&self, max_records: usize, max_bytes: usize) -> bool {
        self.records.len() >= max_records || self.bytes >= max_bytes
    }
}

fn approx_size(v: &Value) -> usize {
    match v {
        Value::Str(s) | Value::Num(s) => s.len() + 4,
        Value::Arr(a) => 2 + a.iter().map(approx_size).sum::<usize>(),
        Value::Obj(o) => 2 + o.iter().map(|(k, x)| k.len() + 4 + approx_size(x)).sum::<usize>(),
        _ => 5,
    }
}

pub trait Records: Send {
    /// Appends up to `max_records` records (or `max_bytes` of data) to the batch. Returns true when exhausted.
    fn next(&mut self, max_records: usize, max_bytes: usize, b: &mut Batch) -> bool;
}

type Stream = Box<dyn Read + Send>;

enum Line {
    Eof,
    Ok,
    TooLong,
}

fn read_line(r: &mut BufReader<Stream>, buf: &mut Vec<u8>, cap: usize) -> io::Result<Line> {
    buf.clear();
    let mut too = false;
    loop {
        let avail = r.fill_buf()?;
        if avail.is_empty() {
            return Ok(if too { Line::TooLong } else if buf.is_empty() { Line::Eof } else { Line::Ok });
        }
        if let Some(p) = avail.iter().position(|&c| c == b'\n') {
            if !too {
                if buf.len() + p > cap {
                    too = true;
                    buf.clear();
                } else {
                    buf.extend_from_slice(&avail[..p]);
                }
            }
            r.consume(p + 1);
            return Ok(if too { Line::TooLong } else { Line::Ok });
        }
        let n = avail.len();
        if !too {
            if buf.len() + n > cap {
                too = true;
                buf.clear();
            } else {
                buf.extend_from_slice(avail);
            }
        }
        r.consume(n);
    }
}

fn bufread(s: Stream) -> BufReader<Stream> {
    BufReader::with_capacity(256 * 1024, s)
}

fn to_object(v: Value) -> Result<Value, String> {
    match v {
        Value::Obj(_) => Ok(v),
        Value::Str(_) => Ok(Value::obj(vec![("text", v)])),
        _ => Err("record is not a JSON object".to_string()),
    }
}

// ---------------------------------------------------------------- JSONL

struct Jsonl {
    r: BufReader<Stream>,
    name: String,
    line: u64,
    cap: usize,
    buf: Vec<u8>,
    done: bool,
}

impl Records for Jsonl {
    fn next(&mut self, max_records: usize, max_bytes: usize, b: &mut Batch) -> bool {
        while !self.done && !b.full(max_records, max_bytes) {
            match read_line(&mut self.r, &mut self.buf, self.cap) {
                Err(e) => {
                    b.err(&self.name, format!("read error after line {}: {} (the rest of this file was not read)", self.line, e));
                    self.done = true;
                }
                Ok(Line::Eof) => self.done = true,
                Ok(Line::TooLong) => {
                    self.line += 1;
                    b.err(&format!("{}:{}", self.name, self.line), format!("line is longer than the {} MiB limit and was skipped", self.cap >> 20));
                }
                Ok(Line::Ok) => {
                    self.line += 1;
                    let mut bytes: &[u8] = &self.buf;
                    if self.line == 1 && bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
                        bytes = &bytes[3..];
                    }
                    if bytes.last() == Some(&b'\r') {
                        bytes = &bytes[..bytes.len() - 1];
                    }
                    if bytes.iter().all(|c| c.is_ascii_whitespace()) {
                        continue;
                    }
                    let wh = format!("{}:{}", self.name, self.line);
                    match std::str::from_utf8(bytes) {
                        Err(_) => b.err(&wh, "invalid UTF-8 (corrupt line skipped)"),
                        Ok(s) => match json::parse(s) {
                            Err(e) => b.err(&wh, e.to_string()),
                            Ok(v) => match to_object(v) {
                                Ok(o) => b.push(&self.name, o),
                                Err(m) => b.err(&wh, m),
                            },
                        },
                    }
                }
            }
        }
        self.done
    }
}

// ---------------------------------------------------------------- JSON (array streaming or single document)

enum Elem {
    Item(Vec<u8>),
    TooLong,
    End,
}

struct JsonDoc {
    r: BufReader<Stream>,
    name: String,
    cap: usize,
    whole_cap: usize,
    started: bool,
    in_array: bool,
    queue: VecDeque<Value>,
    index: u64,
    done: bool,
}

impl JsonDoc {
    fn peek_non_ws(&mut self) -> io::Result<Option<u8>> {
        loop {
            let a = self.r.fill_buf()?;
            if a.is_empty() {
                return Ok(None);
            }
            let skip = a.iter().take_while(|c| c.is_ascii_whitespace()).count();
            if skip < a.len() {
                let c = a[skip];
                self.r.consume(skip);
                return Ok(Some(c));
            }
            let n = a.len();
            self.r.consume(n);
        }
    }

    fn next_elem(&mut self) -> io::Result<Elem> {
        let mut buf: Vec<u8> = Vec::new();
        let mut too = false;
        let (mut started, mut in_str, mut esc, mut depth) = (false, false, false, 0i32);
        loop {
            let a = self.r.fill_buf()?;
            if a.is_empty() {
                return if started { Err(io::Error::new(io::ErrorKind::UnexpectedEof, "JSON array is truncated")) } else { Ok(Elem::End) };
            }
            let mut i = 0;
            let mut finished = false;
            while i < a.len() {
                let c = a[i];
                if !started {
                    if c.is_ascii_whitespace() || c == b',' {
                        i += 1;
                        continue;
                    }
                    if c == b']' {
                        self.r.consume(i + 1);
                        return Ok(Elem::End);
                    }
                    started = true;
                }
                if in_str {
                    if esc {
                        esc = false;
                    } else if c == b'\\' {
                        esc = true;
                    } else if c == b'"' {
                        in_str = false;
                        if depth == 0 {
                            if !too {
                                buf.push(c);
                            }
                            i += 1;
                            finished = true;
                            break;
                        }
                    }
                } else {
                    match c {
                        b'"' => in_str = true,
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => {
                            if depth == 0 {
                                finished = true; // end of array right after a primitive
                                break;
                            }
                            depth -= 1;
                            if depth == 0 {
                                if !too {
                                    buf.push(c);
                                }
                                i += 1;
                                finished = true;
                                break;
                            }
                        }
                        b',' if depth == 0 => {
                            finished = true;
                            break;
                        }
                        _ => {}
                    }
                }
                if !too {
                    if buf.len() >= self.cap {
                        too = true;
                        buf.clear();
                    } else {
                        buf.push(c);
                    }
                }
                i += 1;
            }
            self.r.consume(i);
            if finished {
                return Ok(if too { Elem::TooLong } else { Elem::Item(buf) });
            }
        }
    }
}

impl Records for JsonDoc {
    fn next(&mut self, max_records: usize, max_bytes: usize, b: &mut Batch) -> bool {
        while !b.full(max_records, max_bytes) {
            if let Some(v) = self.queue.pop_front() {
                match to_object(v) {
                    Ok(o) => b.push(&self.name, o),
                    Err(m) => b.err(&self.name, m),
                }
                continue;
            }
            if self.done {
                break;
            }
            if !self.started {
                self.started = true;
                match self.peek_non_ws() {
                    Err(e) => {
                        b.err(&self.name, format!("read error: {}", e));
                        self.done = true;
                    }
                    Ok(None) => self.done = true,
                    Ok(Some(b'[')) => {
                        self.r.consume(1);
                        self.in_array = true;
                    }
                    Ok(Some(_)) => {
                        let mut raw = Vec::new();
                        let lim = self.whole_cap as u64 + 1;
                        let res = (&mut self.r).take(lim).read_to_end(&mut raw);
                        self.done = true;
                        if let Err(e) = res {
                            b.err(&self.name, format!("read error: {}", e));
                            continue;
                        }
                        if raw.len() > self.whole_cap {
                            b.err(&self.name, format!("JSON document is larger than {} MiB; convert it to JSONL or a JSON array of records", self.whole_cap >> 20));
                            continue;
                        }
                        match std::str::from_utf8(&raw).map_err(|_| "invalid UTF-8".to_string()).and_then(|s| json::parse(s).map_err(|e| e.to_string())) {
                            Err(m) => b.err(&self.name, m),
                            Ok(v) => {
                                // {"data": [ ... ]} style wrappers
                                let single_arr = match &v {
                                    Value::Obj(o) if o.len() == 1 => matches!(o[0].1, Value::Arr(_)),
                                    _ => false,
                                };
                                if single_arr {
                                    if let Value::Obj(mut o) = v {
                                        if let Value::Arr(a) = o.remove(0).1 {
                                            self.queue.extend(a);
                                        }
                                    }
                                } else {
                                    self.queue.push_back(v);
                                }
                            }
                        }
                    }
                }
                continue;
            }
            match self.next_elem() {
                Err(e) => {
                    b.err(&self.name, format!("{} (records after element {} were not read)", e, self.index));
                    self.done = true;
                }
                Ok(Elem::End) => self.done = true,
                Ok(Elem::TooLong) => {
                    self.index += 1;
                    b.err(&format!("{}[{}]", self.name, self.index - 1), format!("element is larger than the {} MiB limit and was skipped", self.cap >> 20));
                }
                Ok(Elem::Item(bytes)) => {
                    self.index += 1;
                    let wh = format!("{}[{}]", self.name, self.index - 1);
                    match std::str::from_utf8(&bytes).map_err(|_| "invalid UTF-8".to_string()).and_then(|s| json::parse(s).map_err(|e| e.to_string())) {
                        Err(m) => b.err(&wh, m),
                        Ok(v) => match to_object(v) {
                            Ok(o) => b.push(&self.name, o),
                            Err(m) => b.err(&wh, m),
                        },
                    }
                }
            }
        }
        self.done && self.queue.is_empty()
    }
}

// ---------------------------------------------------------------- CSV / TSV

struct CsvRecords {
    r: BufReader<Stream>,
    name: String,
    cap: usize,
    delim: Option<char>,
    header: Option<Vec<String>>,
    line: u64,
    buf: Vec<u8>,
    done: bool,
}

impl CsvRecords {
    /// Reads one logical row (quoted fields may span lines).
    fn logical(&mut self, b: &mut Batch) -> Option<String> {
        let mut text = String::new();
        let mut quotes = 0usize;
        loop {
            match read_line(&mut self.r, &mut self.buf, self.cap) {
                Err(e) => {
                    b.err(&self.name, format!("read error after line {}: {}", self.line, e));
                    self.done = true;
                    return None;
                }
                Ok(Line::Eof) => {
                    self.done = true;
                    if text.is_empty() {
                        return None;
                    }
                    b.err(&format!("{}:{}", self.name, self.line), "unterminated quoted field at end of file");
                    return None;
                }
                Ok(Line::TooLong) => {
                    self.line += 1;
                    b.err(&format!("{}:{}", self.name, self.line), "row is longer than the limit and was skipped");
                    if text.is_empty() {
                        continue;
                    }
                    return None;
                }
                Ok(Line::Ok) => {
                    self.line += 1;
                    let s = String::from_utf8_lossy(&self.buf).to_string();
                    let s = s.trim_end_matches('\r');
                    if text.is_empty() && s.trim().is_empty() {
                        continue;
                    }
                    quotes += s.matches('"').count();
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(s);
                    if quotes % 2 == 0 {
                        return Some(text);
                    }
                    if text.len() > self.cap {
                        b.err(&format!("{}:{}", self.name, self.line), "quoted field is longer than the limit; row skipped");
                        return None;
                    }
                }
            }
        }
    }
}

impl Records for CsvRecords {
    fn next(&mut self, max_records: usize, max_bytes: usize, b: &mut Batch) -> bool {
        while !self.done && !b.full(max_records, max_bytes) {
            let Some(row_text) = self.logical(b) else { continue };
            if self.header.is_none() {
                let first = row_text.trim_start_matches('\u{feff}');
                let d = self.delim.unwrap_or_else(|| {
                    [',', '\t', ';', '|'].into_iter().max_by_key(|c| first.matches(*c).count()).unwrap_or(',')
                });
                self.delim = Some(d);
                match csv::parse(first, d) {
                    Ok(rows) if !rows.is_empty() => self.header = Some(rows[0].iter().map(|h| h.trim().to_string()).collect()),
                    _ => {
                        b.err(&self.name, "could not read the header row");
                        self.done = true;
                    }
                }
                continue;
            }
            let d = self.delim.unwrap_or(',');
            let wh = format!("{}:{}", self.name, self.line);
            match csv::parse(&row_text, d) {
                Err(m) => b.err(&wh, m),
                Ok(rows) => {
                    let h = self.header.as_ref().unwrap();
                    for row in rows {
                        if row.len() != h.len() {
                            b.err(&wh, format!("expected {} columns, found {}", h.len(), row.len()));
                            continue;
                        }
                        b.push(&self.name, Value::Obj(h.iter().cloned().zip(row.into_iter().map(Value::Str)).collect()));
                    }
                }
            }
        }
        self.done
    }
}

// ---------------------------------------------------------------- plain text / markdown

struct TextLines {
    r: BufReader<Stream>,
    name: String,
    cap: usize,
    paragraphs: bool,
    buf: Vec<u8>,
    done: bool,
    cur: String,
    start_line: u64,
    line: u64,
}

impl Records for TextLines {
    fn next(&mut self, max_records: usize, max_bytes: usize, b: &mut Batch) -> bool {
        while !self.done && !b.full(max_records, max_bytes) {
            match read_line(&mut self.r, &mut self.buf, self.cap) {
                Err(e) => {
                    b.err(&self.name, format!("read error after line {}: {}", self.line, e));
                    self.done = true;
                }
                Ok(Line::Eof) => {
                    self.done = true;
                    if !self.cur.trim().is_empty() {
                        let t = std::mem::take(&mut self.cur);
                        b.push(&self.name, Value::obj(vec![("text", Value::str(t.trim().to_string()))]));
                    }
                }
                Ok(Line::TooLong) => {
                    self.line += 1;
                    b.err(&format!("{}:{}", self.name, self.line), "line is longer than the limit and was skipped");
                }
                Ok(Line::Ok) => {
                    self.line += 1;
                    let s = String::from_utf8_lossy(&self.buf).to_string();
                    let s = s.trim_end_matches('\r').trim_start_matches('\u{feff}').to_string();
                    if !self.paragraphs {
                        if !s.trim().is_empty() {
                            b.push(&self.name, Value::obj(vec![("text", Value::str(s))]));
                        }
                    } else if s.trim().is_empty() {
                        if !self.cur.trim().is_empty() {
                            let t = std::mem::take(&mut self.cur);
                            b.push(&self.name, Value::obj(vec![("text", Value::str(t.trim().to_string()))]));
                        }
                    } else {
                        if self.cur.is_empty() {
                            self.start_line = self.line;
                        }
                        if self.cur.len() + s.len() > self.cap {
                            b.err(&format!("{}:{}", self.name, self.start_line), "paragraph is longer than the limit and was cut");
                            let t = std::mem::take(&mut self.cur);
                            b.push(&self.name, Value::obj(vec![("text", Value::str(t.trim().to_string()))]));
                        }
                        if !self.cur.is_empty() {
                            self.cur.push('\n');
                        }
                        self.cur.push_str(&s);
                    }
                }
            }
        }
        self.done
    }
}

/// Whole-text readers (labelled conversations/fields, HTML): the content is read once under a size cap.
struct Whole {
    name: String,
    stream: Option<Stream>,
    cap: usize,
    kind: WholeKind,
    queue: VecDeque<Value>,
    loaded: bool,
}

enum WholeKind {
    Labelled { mode: String, fields: Vec<String>, extra_roles: Vec<String> },
    Html { markdown: bool },
}

impl Records for Whole {
    fn next(&mut self, max_records: usize, max_bytes: usize, b: &mut Batch) -> bool {
        if !self.loaded {
            self.loaded = true;
            let mut raw = Vec::new();
            let s = self.stream.take().unwrap();
            if let Err(e) = s.take(self.cap as u64 + 1).read_to_end(&mut raw) {
                b.err(&self.name, format!("read error: {}", e));
            }
            if raw.len() > self.cap {
                b.err(&self.name, format!("file is larger than the {} MiB limit for this format", self.cap >> 20));
            } else {
                let text = String::from_utf8_lossy(&raw).to_string();
                match &self.kind {
                    WholeKind::Labelled { mode, fields, extra_roles } => {
                        let q = Value::obj(vec![
                            ("text", Value::str(text)),
                            ("mode", Value::str(mode.clone())),
                            ("fields", Value::Arr(fields.iter().cloned().map(Value::str).collect())),
                            ("extra_roles", Value::Arr(extra_roles.iter().cloned().map(Value::str).collect())),
                        ]);
                        match crate::script::text_records(&q) {
                            Err(m) => b.err(&self.name, m),
                            Ok(r) => {
                                if let Some(w) = r.get("warnings").and_then(|w| w.as_arr()) {
                                    for x in w {
                                        if let Some(s) = x.as_str() {
                                            b.err(&self.name, s);
                                        }
                                    }
                                }
                                if let Some(a) = r.get("records").and_then(|a| a.as_arr()) {
                                    self.queue.extend(a.iter().cloned());
                                }
                            }
                        }
                    }
                    WholeKind::Html { markdown } => {
                        let opts = crate::html::Opts { markdown: *markdown, ..Default::default() };
                        let d = crate::html::extract(&text, &opts);
                        if d.text.trim().is_empty() {
                            b.err(&self.name, "no main text could be extracted from this page");
                        } else {
                            let meta = Value::Obj(d.meta.iter().map(|(k, v)| (k.clone(), Value::str(v.clone()))).collect());
                            let rec = Value::obj(vec![
                                ("text", Value::str(d.text.clone())),
                                ("title", Value::str(d.title.clone())),
                                ("lang", Value::str(d.lang.clone())),
                                ("meta", meta),
                                ("license_hints", Value::Arr(d.license_hints.iter().cloned().map(Value::str).collect())),
                                ("raw_chars", Value::int(d.raw_chars as i64)),
                                ("kept_chars", Value::int(d.kept_chars as i64)),
                            ]);
                            self.queue.push_back(rec);
                        }
                    }
                }
            }
        }
        while !b.full(max_records, max_bytes) {
            match self.queue.pop_front() {
                Some(v) => b.push(&self.name, v),
                None => return true,
            }
        }
        self.queue.is_empty()
    }
}

// ---------------------------------------------------------------- archives

enum Container {
    Zip { path: PathBuf, entries: Vec<ZipEntry>, idx: usize },
    Tar { st: SharedTar },
}

struct ArchiveRecords {
    c: Container,
    lim: Limits,
    count: Arc<AtomicU64>,
    cur: Option<Box<dyn Records>>,
    done: bool,
    skipped: u64,
    note_skipped: bool,
}

fn wanted(name: &str, lim: &Limits) -> bool {
    match &lim.entry {
        Some(e) => name == e || name.ends_with(&format!("/{}", e)),
        None => true,
    }
}

impl Records for ArchiveRecords {
    fn next(&mut self, max_records: usize, max_bytes: usize, b: &mut Batch) -> bool {
        loop {
            if b.full(max_records, max_bytes) {
                return false;
            }
            if let Some(cur) = self.cur.as_mut() {
                if !cur.next(max_records, max_bytes, b) {
                    return false;
                }
                self.cur = None;
            }
            if self.done {
                if self.skipped > 0 && self.note_skipped {
                    self.note_skipped = false;
                    b.err("archive", format!("{} entries were skipped (unsupported type, directory or filtered out)", self.skipped));
                }
                return true;
            }
            match &mut self.c {
                Container::Zip { path, entries, idx } => {
                    if *idx >= entries.len() {
                        self.done = true;
                        continue;
                    }
                    let e = entries[*idx].clone();
                    *idx += 1;
                    if e.name.ends_with('/') || !wanted(&e.name, &self.lim) || supported_ext(&e.name).is_none() {
                        self.skipped += 1;
                        continue;
                    }
                    match archive::zip_open(path, &e, self.lim.max_window, self.count.clone()).and_then(|s| leaf(&e.name, s, &self.lim, None).map_err(bad)) {
                        Ok(r) => self.cur = Some(r),
                        Err(err) => b.err(&e.name, format!("entry skipped: {}", err)),
                    }
                }
                Container::Tar { st } => match archive::tar_next(st) {
                    Err(e) => {
                        b.err("archive", format!("{} (reading stopped here)", e));
                        self.done = true;
                    }
                    Ok(None) => self.done = true,
                    Ok(Some(e)) => {
                        if !e.is_file || !wanted(&e.name, &self.lim) || supported_ext(&e.name).is_none() {
                            self.skipped += 1;
                            continue;
                        }
                        let s: Stream = Box::new(TarData { st: st.clone() });
                        match leaf(&e.name, s, &self.lim, None) {
                            Ok(r) => self.cur = Some(r),
                            Err(m) => b.err(&e.name, format!("entry skipped: {}", m)),
                        }
                    }
                },
            }
        }
    }
}

fn supported_ext(name: &str) -> Option<&'static str> {
    Some(match archive::leaf_ext(name).as_str() {
        "jsonl" | "ndjson" | "jl" => "jsonl",
        "json" => "json",
        "csv" => "csv",
        "tsv" | "tab" => "tsv",
        "txt" | "text" | "md" | "markdown" | "rst" | "log" => "text",
        "html" | "htm" | "xhtml" => "html",
        "parquet" => "parquet",
        "docx" => "docx",
        "pdf" => "pdf",
        "xlsx" => "xlsx",
        "epub" => "epub",
        "odt" | "odp" | "ods" | "fodt" => "odt",
        "pptx" => "pptx",
        "rtf" => "rtf",
        "ipynb" => "ipynb",
        "srt" => "srt",
        "vtt" => "vtt",
        _ => return None,
    })
}

struct Spooled {
    inner: Box<dyn Records>,
    path: PathBuf,
}

impl Records for Spooled {
    fn next(&mut self, a: usize, bb: usize, b: &mut Batch) -> bool {
        self.inner.next(a, bb, b)
    }
}

impl Drop for Spooled {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Builds the reader for one stream according to the file name's extension (content sniffing when there is none).
fn leaf(name: &str, stream: Stream, lim: &Limits, hint: Option<&str>) -> Result<Box<dyn Records>, String> {
    let stream: Stream = Box::new(Limited { inner: stream, left: lim.max_file_bytes, unlimited: lim.max_file_bytes == 0, what: name.to_string() });
    let mut kind = hint.or_else(|| supported_ext(name));
    let mut r = bufread(stream);
    if kind.is_none() {
        let head = r.fill_buf().map_err(|e| e.to_string())?;
        let first = head.iter().copied().find(|c| !c.is_ascii_whitespace());
        kind = Some(match first {
            Some(b'%') if head.starts_with(b"%PDF") => "pdf",
            Some(b'[') => "json",
            Some(b'{') => "jsonl",
            Some(b'<') => "html",
            _ => "text",
        });
    }
    let nm = name.to_string();
    Ok(match kind.unwrap() {
        "jsonl" => Box::new(Jsonl { r, name: nm, line: 0, cap: lim.max_line_bytes, buf: Vec::new(), done: false }),
        "json" => Box::new(JsonDoc { r, name: nm, cap: lim.max_line_bytes, whole_cap: lim.whole_file_cap, started: false, in_array: false, queue: VecDeque::new(), index: 0, done: false }),
        "csv" | "tsv" => Box::new(CsvRecords { r, name: nm, cap: lim.max_line_bytes, delim: if kind == Some("tsv") { Some('\t') } else { lim.csv_delim }, header: None, line: 0, buf: Vec::new(), done: false }),
        "html" => Box::new(Whole { name: nm, stream: Some(Box::new(r)), cap: lim.whole_file_cap, kind: WholeKind::Html { markdown: lim.html_markdown }, queue: VecDeque::new(), loaded: false }),
        "docx" | "pdf" | "epub" | "odt" | "pptx" | "rtf" | "ipynb" | "srt" | "vtt" => {
            let is_pdf = kind == Some("pdf");
            let ext = kind.unwrap_or("docx");
            // A .docx is a ZIP package: spool it to a temporary file (bounded by the file limit), extract the text, read it as text.
            let tmp = lim.tmp_dir.join(format!("mnx_spool_{}_{}.{}", std::process::id(), SPOOL.fetch_add(1, Ordering::Relaxed), ext));
            let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
            let res = io::copy(&mut r, &mut f);
            drop(f);
            let text = match res {
                Ok(_) if is_pdf => crate::pdf::extract(&tmp, lim.whole_file_cap.max(64 << 20)).map(|t| t.text),
                Ok(_) if ext == "docx" => crate::docx::extract(&tmp, lim.whole_file_cap),
                Ok(_) => crate::formats::extract(ext, &tmp, lim.whole_file_cap.max(64 << 20)),
                Err(e) => Err(format!("could not read the document: {}", e)),
            };
            let _ = std::fs::remove_file(&tmp);
            let text = text?;
            let r2 = bufread(Box::new(io::Cursor::new(text.into_bytes())));
            let mode = lim.text_mode.clone();
            if mode == "conversation" || mode == "fields" {
                Box::new(Whole { name: nm, stream: Some(Box::new(r2)), cap: lim.whole_file_cap, kind: WholeKind::Labelled { mode, fields: lim.fields.clone(), extra_roles: lim.extra_roles.clone() }, queue: VecDeque::new(), loaded: false })
            } else {
                Box::new(TextLines { r: r2, name: nm, cap: lim.max_line_bytes, paragraphs: true, buf: Vec::new(), done: false, cur: String::new(), start_line: 0, line: 0 })
            }
        }
        "xlsx" => {
            let tmp = lim.tmp_dir.join(format!("mnx_spool_{}_{}.xlsx", std::process::id(), SPOOL.fetch_add(1, Ordering::Relaxed)));
            let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
            let res = io::copy(&mut r, &mut f);
            drop(f);
            let text = match res {
                Ok(_) => crate::xlsx::extract(&tmp, lim.whole_file_cap.max(64 << 20)),
                Err(e) => Err(format!("could not read the workbook: {}", e)),
            };
            let _ = std::fs::remove_file(&tmp);
            let text = text?;
            let r2 = bufread(Box::new(io::Cursor::new(text.into_bytes())));
            Box::new(CsvRecords { r: r2, name: nm, cap: lim.max_line_bytes, delim: Some(','), header: None, line: 0, buf: Vec::new(), done: false })
        }
        "parquet" => {
            // Parquet needs random access: spool to a temporary file (bounded by the file limit), then read it.
            let tmp = lim.tmp_dir.join(format!("mnx_spool_{}_{}.parquet", std::process::id(), SPOOL.fetch_add(1, Ordering::Relaxed)));
            let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
            let res = io::copy(&mut r, &mut f);
            drop(f);
            if let Err(e) = res {
                let _ = std::fs::remove_file(&tmp);
                return Err(format!("could not extract the parquet file: {}", e));
            }
            match crate::parquet::open(&tmp, nm, lim) {
                Ok(inner) => Box::new(Spooled { inner, path: tmp }),
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(e);
                }
            }
        }
        _ => {
            let mode = lim.text_mode.clone();
            if mode == "conversation" || mode == "fields" {
                Box::new(Whole { name: nm, stream: Some(Box::new(r)), cap: lim.whole_file_cap, kind: WholeKind::Labelled { mode, fields: lim.fields.clone(), extra_roles: lim.extra_roles.clone() }, queue: VecDeque::new(), loaded: false })
            } else {
                Box::new(TextLines { r, name: nm, cap: lim.max_line_bytes, paragraphs: lim.text_split == "paragraphs", buf: Vec::new(), done: false, cur: String::new(), start_line: 0, line: 0 })
            }
        }
    })
}

static SPOOL: AtomicU64 = AtomicU64::new(0);

/// Replays already-read bytes before continuing with the inner reader (used to sniff decompressed streams).
struct Peeked {
    head: Vec<u8>,
    pos: usize,
    inner: Stream,
}

impl Read for Peeked {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        if self.pos < self.head.len() {
            let n = b.len().min(self.head.len() - self.pos);
            b[..n].copy_from_slice(&self.head[self.pos..self.pos + n]);
            self.pos += n;
            return Ok(n);
        }
        self.inner.read(b)
    }
}

fn peek(mut s: Stream, n: usize) -> io::Result<Peeked> {
    let mut head = vec![0u8; n];
    let mut got = 0;
    while got < n {
        let k = s.read(&mut head[got..])?;
        if k == 0 {
            break;
        }
        got += k;
    }
    head.truncate(got);
    Ok(Peeked { head, pos: 0, inner: s })
}

pub struct Opened {
    pub src: Box<dyn Records>,
    pub format: String,
    pub count: Arc<AtomicU64>,
    pub file_size: u64,
    pub warnings: Vec<String>,
}

pub fn open_source(path: &Path, name: &str, lim: &Limits) -> Result<Opened, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("cannot open the file: {}", e))?;
    let file_size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut head = [0u8; 512];
    let mut got = 0;
    while got < head.len() {
        let k = f.read(&mut head[got..]).map_err(|e| e.to_string())?;
        if k == 0 {
            break;
        }
        got += k;
    }
    let kind = archive::sniff(&head[..got]);
    use std::io::Seek;
    f.seek(io::SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let count = Arc::new(AtomicU64::new(0));
    let mut warnings = Vec::new();
    let counted = |f: std::fs::File| -> Stream { Box::new(Counting { inner: BufReader::with_capacity(256 * 1024, f), count: count.clone() }) };
    let is_docx = kind == Kind::Zip && name.to_lowercase().ends_with(".docx");
    let is_xlsx = kind == Kind::Zip && name.to_lowercase().ends_with(".xlsx");
    let zip_doc: Option<&str> = if kind == Kind::Zip {
        let n = name.to_lowercase();
        [("epub", "epub"), ("odt", "odt"), ("odp", "odt"), ("ods", "odt"), ("pptx", "pptx")].iter().find(|(e, _)| n.ends_with(&format!(".{}", e))).map(|(_, k)| *k)
    } else {
        None
    };
    let (src, format): (Box<dyn Records>, String) = match kind {
        Kind::Zip if zip_doc.is_some() => {
            let k = zip_doc.unwrap();
            let r = leaf(name, counted(f), lim, Some(k))?;
            (r, k.to_string())
        }
        Kind::Zip if is_xlsx => {
            let r = leaf(name, counted(f), lim, Some("xlsx"))?;
            (r, "xlsx".to_string())
        }
        Kind::Zip if is_docx => {
            let r = leaf(name, counted(f), lim, Some("docx"))?;
            (r, "docx".to_string())
        }
        Kind::Zip => {
            let entries = archive::zip_entries(&mut f, lim.max_entries).map_err(|e| e.to_string())?;
            let n = entries.len();
            (Box::new(ArchiveRecords { c: Container::Zip { path: path.to_path_buf(), entries, idx: 0 }, lim: lim.clone(), count: count.clone(), cur: None, done: false, skipped: 0, note_skipped: true }), format!("zip ({} entries)", n))
        }
        Kind::Tar => {
            let st = archive::tar_new(counted(f));
            (Box::new(ArchiveRecords { c: Container::Tar { st }, lim: lim.clone(), count: count.clone(), cur: None, done: false, skipped: 0, note_skipped: true }), "tar".to_string())
        }
        Kind::Gzip | Kind::Zstd => {
            let raw = counted(f);
            let dec: Stream = if kind == Kind::Gzip { Box::new(Gunzip::new(raw)) } else { Box::new(Zstd::new(raw, lim.max_window)) };
            let p = peek(dec, 512).map_err(|e| format!("{}", e))?;
            let inner_kind = archive::sniff(&p.head);
            let label = kind.name();
            if inner_kind == Kind::Tar {
                let st = archive::tar_new(Box::new(p));
                (Box::new(ArchiveRecords { c: Container::Tar { st }, lim: lim.clone(), count: count.clone(), cur: None, done: false, skipped: 0, note_skipped: true }), format!("tar.{}", label))
            } else {
                let inner_name = {
                    let l = name.to_lowercase();
                    let stripped = [".gz", ".zst", ".zstd"].iter().find_map(|s| l.strip_suffix(s)).map(|s| name[..s.len()].to_string()).unwrap_or_else(|| name.to_string());
                    stripped
                };
                if supported_ext(&inner_name).is_none() {
                    warnings.push(format!("no known extension on '{}'; content was sniffed", inner_name));
                }
                let r = leaf(&inner_name, Box::new(p), lim, None)?;
                (r, format!("{} → {}", label, supported_ext(&inner_name).unwrap_or("auto")))
            }
        }
        Kind::Parquet => {
            let r = crate::parquet::open(path, name.to_string(), lim)?;
            // progress for parquet is reported by the reader itself through the shared counter
            (r, "parquet".to_string())
        }
        Kind::Plain => {
            let r = leaf(name, counted(f), lim, None)?;
            (r, supported_ext(name).unwrap_or("auto").to_string())
        }
    };
    Ok(Opened { src, format, count, file_size, warnings })
}

// ---------------------------------------------------------------- handles

struct Slot {
    src: Box<dyn Records>,
    count: Arc<AtomicU64>,
    file_size: u64,
    total_records: u64,
    errors_total: u64,
    done: bool,
    skip: u64, // records to discard before returning data (resume after an interrupted import)
}

static REG: OnceLock<Mutex<(u64, HashMap<u64, Slot>)>> = OnceLock::new();

fn reg() -> &'static Mutex<(u64, HashMap<u64, Slot>)> {
    REG.get_or_init(|| Mutex::new((0, HashMap::new())))
}

fn s_of<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str())
}

fn n_of(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(|x| x.as_f64())
}

fn strs(v: &Value, k: &str) -> Vec<String> {
    v.get(k).and_then(|a| a.as_arr()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default()
}

pub fn limits_from(v: &Value) -> Limits {
    let mut l = Limits::default();
    let o = v.get("limits").cloned().unwrap_or(Value::Null);
    if let Some(n) = n_of(&o, "max_line_mb") {
        l.max_line_bytes = (n.clamp(1.0, 512.0) as usize) << 20;
    }
    if let Some(n) = n_of(&o, "max_file_mb") {
        l.max_file_bytes = if n <= 0.0 { 0 } else { (n as u64) << 20 };
    }
    if let Some(n) = n_of(&o, "max_window_mb") {
        l.max_window = (n.clamp(1.0, 2048.0) as usize) << 20;
    }
    if let Some(n) = n_of(&o, "whole_file_mb") {
        l.whole_file_cap = (n.clamp(1.0, 1024.0) as usize) << 20;
    }
    if let Some(n) = n_of(&o, "max_entries") {
        l.max_entries = n.clamp(1.0, 10_000_000.0) as usize;
    }
    if let Some(t) = s_of(v, "tmp_dir") {
        l.tmp_dir = PathBuf::from(t);
    }
    l.csv_delim = s_of(v, "csv_delimiter").and_then(|d| if d == "\\t" { Some('\t') } else { d.chars().next() });
    if let Some(m) = s_of(v, "text_mode") {
        l.text_mode = m.to_string();
    }
    if let Some(m) = s_of(v, "text_split") {
        l.text_split = m.to_string();
    }
    l.fields = strs(v, "fields");
    l.extra_roles = strs(v, "extra_roles");
    l.entry = s_of(v, "entry").map(|s| s.to_string()).filter(|s| !s.is_empty());
    l.html_markdown = v.get("html_markdown").and_then(|x| x.as_bool()).unwrap_or(true);
    l
}

pub fn op_open(v: &Value) -> Result<Value, String> {
    let path = s_of(v, "path").ok_or("missing 'path'")?;
    let name = s_of(v, "name").filter(|n| !n.is_empty()).unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path));
    let lim = limits_from(v);
    let o = open_source(Path::new(path), name, &lim)?;
    let mut g = reg().lock().map_err(|_| "registry poisoned".to_string())?;
    g.0 += 1;
    let id = g.0;
    g.1.insert(id, Slot { src: o.src, count: o.count, file_size: o.file_size, total_records: 0, errors_total: 0, done: false, skip: n_of(v, "skip_records").unwrap_or(0.0).max(0.0) as u64 });
    Ok(Value::obj(vec![
        ("handle", Value::int(id as i64)),
        ("format", Value::str(o.format)),
        ("file_size", Value::int(o.file_size as i64)),
        ("warnings", Value::Arr(o.warnings.into_iter().map(Value::str).collect())),
    ]))
}

pub fn op_next(v: &Value) -> Result<Value, String> {
    let h = n_of(v, "handle").ok_or("missing 'handle'")? as u64;
    let max_records = n_of(v, "max_records").unwrap_or(1000.0).clamp(1.0, 100_000.0) as usize;
    let max_bytes = (n_of(v, "max_mb").unwrap_or(8.0).clamp(0.1, 256.0) * 1048576.0) as usize;
    // take the slot out so other handles are not blocked while this batch is read
    let mut slot = {
        let mut g = reg().lock().map_err(|_| "registry poisoned".to_string())?;
        g.1.remove(&h).ok_or("unknown or closed handle")?
    };
    let mut b = Batch::default();
    // Resume: discard records that an earlier run already imported (same file, same order).
    let mut budget: u64 = 200_000; // keep each call short so the app can show progress while skipping
    while slot.skip > 0 && !slot.done && budget > 0 {
        let mut tb = Batch::default();
        let want = slot.skip.min(5000) as usize;
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| slot.src.next(want, 64 << 20, &mut tb)));
        match r {
            Ok(done) => slot.done = done,
            Err(_) => {
                b.err("reader", "internal error while reading; the file was stopped");
                slot.done = true;
            }
        }
        let n = tb.records.len() as u64;
        budget = budget.saturating_sub(n.max(1));
        slot.total_records += n.min(slot.skip);
        if n > slot.skip {
            let k = slot.skip as usize;
            let rest: Vec<Value> = tb.records.split_off(k);
            let rest_src: Vec<String> = tb.sources.split_off(k);
            for (rv, rs) in rest.into_iter().zip(rest_src) {
                b.push(&rs, rv);
            }
            slot.skip = 0;
        } else {
            slot.skip -= n;
        }
    }
    if !slot.done && slot.skip == 0 {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| slot.src.next(max_records, max_bytes, &mut b)));
        match r {
            Ok(done) => slot.done = done,
            Err(_) => {
                b.err("reader", "internal error while reading; the file was stopped");
                slot.done = true;
            }
        }
    }
    slot.total_records += b.records.len() as u64;
    slot.errors_total += b.errors.len() as u64 + b.errors_dropped;
    let out = Value::obj(vec![
        ("records", Value::Arr(std::mem::take(&mut b.records))),
        ("sources", Value::Arr(b.sources.iter().cloned().map(Value::str).collect())),
        ("errors", Value::Arr(b.errors.iter().map(|(w, m)| Value::obj(vec![("where", Value::str(w.clone())), ("message", Value::str(m.clone()))])).collect())),
        ("errors_dropped", Value::int(b.errors_dropped as i64)),
        ("done", Value::Bool(slot.done)),
        ("file_pos", Value::int(slot.count.load(Ordering::Relaxed) as i64)),
        ("file_size", Value::int(slot.file_size as i64)),
        ("total_records", Value::int(slot.total_records as i64)),
        ("errors_total", Value::int(slot.errors_total as i64)),
    ]);
    let mut g = reg().lock().map_err(|_| "registry poisoned".to_string())?;
    g.1.insert(h, slot);
    Ok(out)
}

pub fn op_close(v: &Value) -> Result<Value, String> {
    let h = n_of(v, "handle").ok_or("missing 'handle'")? as u64;
    let mut g = reg().lock().map_err(|_| "registry poisoned".to_string())?;
    let had = g.1.remove(&h).is_some();
    Ok(Value::obj(vec![("closed", Value::Bool(had))]))
}

/// op "probe": cheap format detection plus, for archives, the entry list.
pub fn op_probe(v: &Value) -> Result<Value, String> {
    let path = s_of(v, "path").ok_or("missing 'path'")?;
    let name = s_of(v, "name").unwrap_or(path);
    let lim = limits_from(v);
    let mut f = std::fs::File::open(path).map_err(|e| format!("cannot open the file: {}", e))?;
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut head = [0u8; 512];
    let n = f.read(&mut head).map_err(|e| e.to_string())?;
    let kind = archive::sniff(&head[..n]);
    let mut entries: Vec<Value> = Vec::new();
    let mut note: Option<String> = None;
    if kind == Kind::Zip {
        use std::io::Seek;
        f.seek(io::SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        match archive::zip_entries(&mut f, lim.max_entries.min(5000)) {
            Ok(es) => {
                for e in es {
                    entries.push(Value::obj(vec![("name", Value::str(e.name.clone())), ("size", Value::int(e.size as i64)), ("supported", Value::Bool(supported_ext(&e.name).is_some() && !e.name.ends_with('/')))]));
                }
            }
            Err(e) => note = Some(e.to_string()),
        }
    }
    let leaf_kind = supported_ext(name).unwrap_or("");
    let mut out = vec![
        ("kind", Value::str(kind.name())),
        ("size", Value::int(size as i64)),
        ("extension_format", Value::str(leaf_kind)),
        ("entries", Value::Arr(entries)),
    ];
    if let Some(n) = note {
        out.push(("note", Value::str(n)));
    }
    Ok(Value::obj(out))
}

/// op "file_hash": streaming SHA-256 / CRC-32 of a file.
pub fn op_file_hash(v: &Value) -> Result<Value, String> {
    let path = s_of(v, "path").ok_or("missing 'path'")?;
    let algo = s_of(v, "algo").unwrap_or("sha256");
    let mut f = std::fs::File::open(path).map_err(|e| format!("cannot open the file: {}", e))?;
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    let hex;
    if algo == "crc32" {
        let mut c = crate::hash::Crc32::new();
        loop {
            let n = f.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            c.update(&buf[..n]);
            total += n as u64;
        }
        hex = format!("{:08x}", c.finish());
    } else {
        let mut s = crate::hash::Sha256::new();
        loop {
            let n = f.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            s.update(&buf[..n]);
            total += n as u64;
        }
        hex = s.finish_hex();
    }
    Ok(Value::obj(vec![("hex", Value::str(hex)), ("bytes", Value::int(total as i64))]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mnx_rd_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn drain(path: &Path, name: &str, lim: &Limits) -> (Vec<Value>, Vec<String>, Vec<(String, String)>) {
        let o = open_source(path, name, lim).unwrap();
        let mut src = o.src;
        let (mut recs, mut srcs, mut errs) = (vec![], vec![], vec![]);
        loop {
            let mut b = Batch::default();
            let done = src.next(7, 1 << 20, &mut b);
            recs.extend(b.records);
            srcs.extend(b.sources);
            errs.extend(b.errors);
            if done {
                break;
            }
        }
        (recs, srcs, errs)
    }

    #[test]
    fn jsonl_with_damage() {
        let d = tmpdir("jl");
        let mut s = String::new();
        for i in 0..50 {
            s.push_str(&format!("{{\"i\": {}}}\n", i));
            if i == 10 {
                s.push_str("{broken\n\n");
            }
            if i == 20 {
                s.push_str(&format!("{{\"big\": \"{}\"}}\n", "x".repeat(3000)));
            }
        }
        let p = d.join("a.jsonl");
        std::fs::write(&p, s).unwrap();
        let lim = Limits { max_line_bytes: 1000, ..Default::default() };
        let (recs, _, errs) = drain(&p, "a.jsonl", &lim);
        assert_eq!(recs.len(), 50);
        assert_eq!(errs.len(), 2);
        assert!(errs.iter().any(|e| e.1.contains("longer")));
    }

    #[test]
    fn json_array_streaming_and_wrappers() {
        let d = tmpdir("js");
        let p = d.join("a.json");
        std::fs::write(&p, r#"  [ {"a":1}, {"a":"x]\"y,"}, "plain string", {"n":[1,2,{"z":[]}]}, 5, {"a":3} ] "#).unwrap();
        let (recs, _, errs) = drain(&p, "a.json", &Limits::default());
        assert_eq!(recs.len(), 5);
        assert_eq!(recs[2].get("text").unwrap().as_str().unwrap(), "plain string");
        assert_eq!(errs.len(), 1); // the bare number
        std::fs::write(&p, r#"{"data": [{"q":1},{"q":2}]}"#).unwrap();
        assert_eq!(drain(&p, "a.json", &Limits::default()).0.len(), 2);
        std::fs::write(&p, r#"{"q": 1}"#).unwrap();
        assert_eq!(drain(&p, "a.json", &Limits::default()).0.len(), 1);
        std::fs::write(&p, r#"[{"a":1},{"a":2"#).unwrap();
        let (recs, _, errs) = drain(&p, "a.json", &Limits::default());
        assert_eq!(recs.len(), 1);
        assert!(!errs.is_empty());
    }

    #[test]
    fn csv_quotes_and_delimiters() {
        let d = tmpdir("csv");
        let p = d.join("a.csv");
        std::fs::write(&p, "\u{feff}name;note\nAnn;\"two\nlines; here\"\nBob;ok\nbad\n").unwrap();
        let (recs, _, errs) = drain(&p, "a.csv", &Limits::default());
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].get("note").unwrap().as_str().unwrap(), "two\nlines; here");
        assert_eq!(errs.len(), 1);
    }

    #[test]
    fn text_modes() {
        let d = tmpdir("txt");
        let p = d.join("a.md");
        std::fs::write(&p, "## User\nhi there\n\n## Assistant\nhello\n").unwrap();
        let lim = Limits { text_mode: "conversation".into(), ..Default::default() };
        let (recs, _, _) = drain(&p, "a.md", &lim);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].get("messages").unwrap().as_arr().unwrap().len(), 2);
        let lim = Limits { text_split: "paragraphs".into(), ..Default::default() };
        let (recs, _, _) = drain(&p, "a.md", &lim);
        assert_eq!(recs.len(), 2);
    }

    #[test]
    fn gz_zip_tar_zst_pipeline() {
        let d = tmpdir("arch");
        std::fs::write(d.join("a.jsonl"), "{\"x\":1}\n{\"x\":2}\n").unwrap();
        std::fs::write(d.join("b.csv"), "h1,h2\n1,2\n3,4\n").unwrap();
        std::fs::write(d.join("c.html"), "<html><body><article><h1>T</h1><p>Some longer paragraph of text, with commas, to be kept as content here.</p></article></body></html>").unwrap();
        std::fs::write(d.join("skip.bin"), [0u8, 1, 2]).unwrap();
        let run = |cmd: &str, args: &[&str]| Command::new(cmd).current_dir(&d).args(args).status().map(|s| s.success()).unwrap_or(false);
        let mut checked = 0;
        if run("gzip", &["-k", "a.jsonl"]) {
            let (recs, _, _) = drain(&d.join("a.jsonl.gz"), "a.jsonl.gz", &Limits::default());
            assert_eq!(recs.len(), 2);
            checked += 1;
        }
        if run("zip", &["-q", "t.zip", "a.jsonl", "b.csv", "c.html", "skip.bin"]) {
            let (recs, srcs, errs) = drain(&d.join("t.zip"), "t.zip", &Limits::default());
            assert_eq!(recs.len(), 5, "{:?}", errs);
            assert!(srcs.contains(&"b.csv".to_string()) && srcs.contains(&"c.html".to_string()));
            assert!(errs.iter().any(|e| e.1.contains("skipped")));
            let lim = Limits { entry: Some("b.csv".into()), ..Default::default() };
            assert_eq!(drain(&d.join("t.zip"), "t.zip", &lim).0.len(), 2);
            checked += 1;
        }
        if run("tar", &["-czf", "t.tar.gz", "a.jsonl", "b.csv"]) {
            let (recs, _, _) = drain(&d.join("t.tar.gz"), "t.tar.gz", &Limits::default());
            assert_eq!(recs.len(), 4);
            checked += 1;
        }
        // zstd: the bundled vector is JSONL-sized text; just make sure the pipeline opens it as a stream
        let z = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join("jsonl_l9.zst");
        let (recs, _, errs) = drain(&z, "jsonl_l9.jsonl.zst", &Limits::default());
        assert_eq!(recs.len(), 6000, "{:?}", errs.first());
        assert!(checked >= 1);
    }

    #[test]
    fn handles_and_limits() {
        let d = tmpdir("h");
        let p = d.join("a.jsonl");
        std::fs::write(&p, "{\"x\":1}\n".repeat(25)).unwrap();
        let o = op_open(&json::parse(&format!("{{\"path\":\"{}\",\"name\":\"a.jsonl\"}}", p.display())).unwrap()).unwrap();
        let h = o.get("handle").unwrap().as_f64().unwrap();
        let mut total = 0;
        loop {
            let r = op_next(&json::parse(&format!("{{\"handle\":{},\"max_records\":10}}", h)).unwrap()).unwrap();
            total += r.get("records").unwrap().as_arr().unwrap().len();
            if r.get("done").unwrap().as_bool().unwrap() {
                break;
            }
        }
        assert_eq!(total, 25);
        assert!(op_close(&json::parse(&format!("{{\"handle\":{}}}", h)).unwrap()).unwrap().get("closed").unwrap().as_bool().unwrap());
        assert!(op_next(&json::parse(&format!("{{\"handle\":{}}}", h)).unwrap()).is_err());
        // decompression bomb guard
        let z = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join("repeat_l3.zst");
        let lim = Limits { max_file_bytes: 100_000, ..Default::default() };
        let (recs, _, errs) = drain(&z, "repeat.txt.zst", &lim);
        assert!(recs.is_empty() && errs.iter().any(|e| e.1.contains("limit")), "{:?} {:?}", recs.len(), errs);
        let h = op_file_hash(&json::parse(&format!("{{\"path\":\"{}\"}}", p.display())).unwrap()).unwrap();
        assert_eq!(h.get("bytes").unwrap().as_f64().unwrap(), 200.0);
    }
}

#[cfg(test)]
mod resume_tests {
    use super::*;
    use crate::json::parse;

    fn drain(handle: f64) -> Vec<Value> {
        let mut all = Vec::new();
        loop {
            let r = op_next(&parse(&format!("{{\"handle\":{},\"max_records\":7}}", handle)).unwrap()).unwrap();
            all.extend(r.get("records").unwrap().as_arr().unwrap().iter().cloned());
            if r.get("done").unwrap().as_bool().unwrap() {
                break;
            }
        }
        op_close(&parse(&format!("{{\"handle\":{}}}", handle)).unwrap()).unwrap();
        all
    }

    #[test]
    fn skip_records_resumes_exactly() {
        let dir = std::env::temp_dir().join(format!("mnx_resume_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("r.jsonl");
        let body: String = (0..100).map(|i| format!("{{\"n\":{}}}\n", i)).collect();
        std::fs::write(&f, body).unwrap();
        for skip in [0, 1, 7, 50, 99, 100, 250] {
            let q = parse(&format!("{{\"path\":\"{}\",\"skip_records\":{}}}", f.display(), skip)).unwrap();
            let h = op_open(&q).unwrap().get("handle").unwrap().as_f64().unwrap();
            let got = drain(h);
            let want = 100usize.saturating_sub(skip);
            assert_eq!(got.len(), want, "skip {}", skip);
            if want > 0 {
                assert_eq!(got[0].get("n").unwrap().as_f64().unwrap() as usize, skip.min(100));
            }
        }
    }

    #[test]
    fn docx_is_read_as_paragraph_records() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/sample.docx");
        let q = parse(&format!("{{\"path\":\"{}\",\"name\":\"sample.docx\",\"text_split\":\"paragraphs\"}}", p.display())).unwrap();
        let o = op_open(&q).unwrap();
        assert_eq!(o.get("format").unwrap().as_str().unwrap(), "docx");
        let recs = drain(o.get("handle").unwrap().as_f64().unwrap());
        let all: String = recs.iter().map(|r| r.get("text").unwrap().as_str().unwrap().to_string()).collect::<Vec<_>>().join("|");
        assert!(all.contains("# Sample document") && all.contains("First paragraph") && all.contains("- list item") && all.contains("cell A") && all.contains("بالعربية"), "{}", all);
    }

    #[test]
    fn pdf_is_read_as_text_records() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/sample.pdf");
        let q = parse(&format!("{{\"path\":\"{}\",\"name\":\"sample.pdf\",\"text_split\":\"paragraphs\"}}", p.display())).unwrap();
        let o = op_open(&q).unwrap();
        let recs = drain(o.get("handle").unwrap().as_f64().unwrap());
        let all: String = recs.iter().map(|r| r.get("text").unwrap().as_str().unwrap().to_string()).collect::<Vec<_>>().join("|");
        assert!(all.contains("MNX DataForge test document") && all.contains("Page two content"), "{}", all);
    }

    #[test]
    fn xlsx_is_read_as_rows() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/sample.xlsx");
        let q = parse(&format!("{{\"path\":\"{}\",\"name\":\"sample.xlsx\"}}", p.display())).unwrap();
        let o = op_open(&q).unwrap();
        assert_eq!(o.get("format").unwrap().as_str().unwrap(), "xlsx");
        let recs = drain(o.get("handle").unwrap().as_f64().unwrap());
        assert_eq!(recs.len(), 2);
        assert!(recs[0].to_json().contains("\"name\":\"Ali\""), "{}", recs[0].to_json());
    }
}

