//! PDF text extraction (no dependencies). Scans the file for objects (so slightly damaged files still work), reads object
//! streams, walks the page tree in order, decodes content streams (Flate, ASCIIHex, ASCII85) and maps text through
//! ToUnicode CMaps or simple encodings. Paragraphs are rebuilt from line positions.
//! Not supported (reported, never a crash): encrypted files, scanned images (no OCR), LZW/JPX/CCITT streams, vertical text.

use crate::inflate::Inflate;
use std::collections::{HashMap, HashSet};
use std::io::Read;

#[derive(Clone, Debug)]
enum P {
    Null,
    Bool(bool),
    Num(f64),
    Name(String),
    Str(Vec<u8>),
    Arr(Vec<P>),
    Dict(Vec<(String, P)>),
    Ref(u32),
    Op(String),
}

impl P {
    fn get(&self, k: &str) -> Option<&P> {
        match self {
            P::Dict(d) => d.iter().find(|(n, _)| n == k).map(|(_, v)| v),
            _ => None,
        }
    }
    fn name(&self) -> Option<&str> {
        match self {
            P::Name(n) => Some(n),
            _ => None,
        }
    }
    fn num(&self) -> Option<f64> {
        match self {
            P::Num(n) => Some(*n),
            _ => None,
        }
    }
}

struct Parser<'a> {
    d: &'a [u8],
    p: usize,
}

fn is_ws(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}
fn is_delim(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

impl<'a> Parser<'a> {
    fn new(d: &'a [u8], p: usize) -> Self {
        Parser { d, p }
    }
    fn skip(&mut self) {
        while self.p < self.d.len() {
            let b = self.d[self.p];
            if is_ws(b) {
                self.p += 1;
            } else if b == b'%' {
                while self.p < self.d.len() && self.d[self.p] != b'\n' && self.d[self.p] != b'\r' {
                    self.p += 1;
                }
            } else {
                break;
            }
        }
    }
    fn token_end(&self) -> usize {
        let mut q = self.p;
        while q < self.d.len() && !is_ws(self.d[q]) && !is_delim(self.d[q]) {
            q += 1;
        }
        q
    }
    fn parse(&mut self) -> Option<P> {
        self.parse_depth(0)
    }
    fn parse_depth(&mut self, depth: usize) -> Option<P> {
        if depth > 40 {
            return None;
        }
        self.skip();
        let b = *self.d.get(self.p)?;
        match b {
            b'/' => {
                self.p += 1;
                let e = self.token_end();
                let raw = &self.d[self.p..e];
                self.p = e;
                let mut s = Vec::with_capacity(raw.len());
                let mut i = 0;
                while i < raw.len() {
                    if raw[i] == b'#' && i + 2 < raw.len() + 0 && i + 2 <= raw.len() - 1 + 0 {
                        if let Ok(v) = u8::from_str_radix(&String::from_utf8_lossy(&raw[i + 1..i + 3]), 16) {
                            s.push(v);
                            i += 3;
                            continue;
                        }
                    }
                    s.push(raw[i]);
                    i += 1;
                }
                Some(P::Name(String::from_utf8_lossy(&s).to_string()))
            }
            b'(' => {
                self.p += 1;
                let mut out = Vec::new();
                let mut depth_p = 1;
                while self.p < self.d.len() {
                    let c = self.d[self.p];
                    self.p += 1;
                    match c {
                        b'\\' => {
                            let n = *self.d.get(self.p).unwrap_or(&b' ');
                            self.p += 1;
                            match n {
                                b'n' => out.push(b'\n'),
                                b'r' => out.push(b'\r'),
                                b't' => out.push(b'\t'),
                                b'b' => out.push(8),
                                b'f' => out.push(12),
                                b'0'..=b'7' => {
                                    let mut v = (n - b'0') as u32;
                                    for _ in 0..2 {
                                        match self.d.get(self.p) {
                                            Some(&o) if (b'0'..=b'7').contains(&o) => {
                                                v = v * 8 + (o - b'0') as u32;
                                                self.p += 1;
                                            }
                                            _ => break,
                                        }
                                    }
                                    out.push(v as u8);
                                }
                                b'\r' => {
                                    if self.d.get(self.p) == Some(&b'\n') {
                                        self.p += 1;
                                    }
                                }
                                b'\n' => {}
                                other => out.push(other),
                            }
                        }
                        b'(' => {
                            depth_p += 1;
                            out.push(c);
                        }
                        b')' => {
                            depth_p -= 1;
                            if depth_p == 0 {
                                break;
                            }
                            out.push(c);
                        }
                        _ => out.push(c),
                    }
                }
                Some(P::Str(out))
            }
            b'<' => {
                if self.d.get(self.p + 1) == Some(&b'<') {
                    self.p += 2;
                    let mut items = Vec::new();
                    loop {
                        self.skip();
                        if self.p >= self.d.len() {
                            break;
                        }
                        if self.d[self.p] == b'>' && self.d.get(self.p + 1) == Some(&b'>') {
                            self.p += 2;
                            break;
                        }
                        let k = match self.parse_depth(depth + 1) {
                            Some(P::Name(n)) => n,
                            Some(_) => continue,
                            None => break,
                        };
                        let v = self.parse_depth(depth + 1).unwrap_or(P::Null);
                        items.push((k, v));
                    }
                    Some(P::Dict(items))
                } else {
                    self.p += 1;
                    let mut hex = Vec::new();
                    while self.p < self.d.len() && self.d[self.p] != b'>' {
                        let c = self.d[self.p];
                        if c.is_ascii_hexdigit() {
                            hex.push(c);
                        }
                        self.p += 1;
                    }
                    self.p += 1;
                    if hex.len() % 2 == 1 {
                        hex.push(b'0');
                    }
                    Some(P::Str(hex.chunks(2).map(|h| u8::from_str_radix(std::str::from_utf8(h).unwrap_or("0"), 16).unwrap_or(0)).collect()))
                }
            }
            b'[' => {
                self.p += 1;
                let mut items = Vec::new();
                loop {
                    self.skip();
                    if self.p >= self.d.len() {
                        break;
                    }
                    if self.d[self.p] == b']' {
                        self.p += 1;
                        break;
                    }
                    match self.parse_depth(depth + 1) {
                        Some(v) => items.push(v),
                        None => break,
                    }
                }
                Some(P::Arr(items))
            }
            b']' | b'>' | b')' | b'{' | b'}' => {
                self.p += 1;
                Some(P::Op((b as char).to_string()))
            }
            b'\'' | b'"' => {
                self.p += 1;
                Some(P::Op((b as char).to_string()))
            }
            _ => {
                let e = self.token_end();
                if e == self.p {
                    self.p += 1;
                    return Some(P::Null);
                }
                let tok = &self.d[self.p..e];
                self.p = e;
                let s = std::str::from_utf8(tok).unwrap_or("");
                if let Ok(n) = s.parse::<f64>() {
                    // possible indirect reference "N G R"
                    if n >= 0.0 && n.fract() == 0.0 && s.chars().all(|c| c.is_ascii_digit()) {
                        let save = self.p;
                        self.skip();
                        let e2 = self.token_end();
                        let g = std::str::from_utf8(&self.d[self.p..e2]).unwrap_or("");
                        if !g.is_empty() && g.len() < 6 && g.chars().all(|c| c.is_ascii_digit()) {
                            self.p = e2;
                            self.skip();
                            if self.d.get(self.p) == Some(&b'R') && self.d.get(self.p + 1).map_or(true, |&c| is_ws(c) || is_delim(c)) {
                                self.p += 1;
                                return Some(P::Ref(n as u32));
                            }
                        }
                        self.p = save;
                    }
                    return Some(P::Num(n));
                }
                Some(match s {
                    "true" => P::Bool(true),
                    "false" => P::Bool(false),
                    "null" => P::Null,
                    other => P::Op(other.to_string()),
                })
            }
        }
    }
}

struct Obj {
    v: P,
    stream: Option<(usize, usize)>,
}

struct Doc<'a> {
    d: &'a [u8],
    objs: HashMap<u32, Obj>,
    extra: HashMap<u32, P>, // objects living inside object streams
    cap: usize,
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= hay.len() {
        return None;
    }
    hay[from..].windows(needle.len()).position(|w| w == needle).map(|i| i + from)
}

fn flate(data: &[u8], cap: usize) -> Vec<u8> {
    let body = if data.len() >= 2 && (data[0] & 0x0f) == 8 { &data[2..] } else { data };
    let mut out = Vec::new();
    let _ = Inflate::new(body).take(cap as u64).read_to_end(&mut out); // a damaged tail keeps what was decoded
    out
}

fn ascii85(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut grp = [0u32; 5];
    let mut n = 0;
    let mut it = data.iter().copied().skip_while(|&b| is_ws(b));
    if data.starts_with(b"<~") {
        it.next();
        it.next();
    }
    for b in it {
        match b {
            b'~' => break,
            b if is_ws(b) => continue,
            b'z' if n == 0 => out.extend_from_slice(&[0, 0, 0, 0]),
            b'!'..=b'u' => {
                grp[n] = (b - b'!') as u32;
                n += 1;
                if n == 5 {
                    let v = grp.iter().fold(0u32, |a, &x| a.wrapping_mul(85).wrapping_add(x));
                    out.extend_from_slice(&v.to_be_bytes());
                    n = 0;
                }
            }
            _ => {}
        }
    }
    if n > 1 {
        for g in grp.iter_mut().skip(n) {
            *g = 84;
        }
        let v = grp.iter().fold(0u32, |a, &x| a.wrapping_mul(85).wrapping_add(x));
        out.extend_from_slice(&v.to_be_bytes()[..n - 1]);
    }
    out
}

fn png_predict(data: &[u8], columns: usize, colors: usize, bpc: usize) -> Vec<u8> {
    let bpp = ((colors * bpc) + 7) / 8;
    let row = (columns * colors * bpc + 7) / 8;
    if row == 0 {
        return data.to_vec();
    }
    let mut out: Vec<u8> = Vec::with_capacity(data.len());
    let mut prev = vec![0u8; row];
    for chunk in data.chunks(row + 1) {
        if chunk.len() < 2 {
            break;
        }
        let ft = chunk[0];
        let mut cur = chunk[1..].to_vec();
        cur.resize(row, 0);
        for i in 0..row {
            let a = if i >= bpp { cur[i - bpp] as i32 } else { 0 };
            let b = prev[i] as i32;
            let c = if i >= bpp { prev[i - bpp] as i32 } else { 0 };
            let add = match ft {
                1 => a,
                2 => b,
                3 => (a + b) / 2,
                4 => {
                    let p = a + b - c;
                    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                    if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c }
                }
                _ => 0,
            };
            cur[i] = (cur[i] as i32 + add) as u8;
        }
        out.extend_from_slice(&cur);
        prev = cur;
    }
    out
}

impl<'a> Doc<'a> {
    fn scan(d: &'a [u8], cap: usize) -> Doc<'a> {
        let mut doc = Doc { d, objs: HashMap::new(), extra: HashMap::new(), cap };
        let mut i = 0usize;
        while let Some(at) = find(d, b"obj", i) {
            i = at + 3;
            let ok_after = d.get(at + 3).map_or(true, |&c| is_ws(c) || is_delim(c));
            if !ok_after || at < 4 || !is_ws(d[at - 1]) && !d[at - 1].is_ascii_digit() {
                continue;
            }
            // "<num> <gen> obj"
            let mut q = at;
            while q > 0 && is_ws(d[q - 1]) {
                q -= 1;
            }
            let ge = q;
            while q > 0 && d[q - 1].is_ascii_digit() {
                q -= 1;
            }
            if q == ge {
                continue;
            }
            while q > 0 && is_ws(d[q - 1]) {
                q -= 1;
            }
            let ne = q;
            while q > 0 && d[q - 1].is_ascii_digit() {
                q -= 1;
            }
            if q == ne || ne - q > 9 {
                continue;
            }
            let num: u32 = match std::str::from_utf8(&d[q..ne]).ok().and_then(|s| s.parse().ok()) {
                Some(n) => n,
                None => continue,
            };
            if at >= 3 && &d[at.saturating_sub(3)..at] == b"end" {
                continue;
            }
            let mut ps = Parser::new(d, at + 3);
            let v = match ps.parse() {
                Some(v) => v,
                None => continue,
            };
            ps.skip();
            let mut stream = None;
            let mut end = ps.p;
            if d[ps.p.min(d.len())..].starts_with(b"stream") {
                let mut s = ps.p + 6;
                if d.get(s) == Some(&b'\r') {
                    s += 1;
                }
                if d.get(s) == Some(&b'\n') {
                    s += 1;
                }
                let mut e = None;
                if let Some(len) = v.get("Length").and_then(|l| l.num()) {
                    let le = s + len as usize;
                    if le <= d.len() {
                        let mut t = le;
                        while t < d.len() && is_ws(d[t]) {
                            t += 1;
                        }
                        if d[t.min(d.len())..].starts_with(b"endstream") {
                            e = Some(le);
                        }
                    }
                }
                let e = match e.or_else(|| find(d, b"endstream", s)) {
                    Some(e) => e,
                    None => d.len(),
                };
                let mut e2 = e;
                if e2 > s && d[e2 - 1] == b'\n' {
                    e2 -= 1;
                }
                if e2 > s && d[e2 - 1] == b'\r' {
                    e2 -= 1;
                }
                stream = Some((s, if e2 >= s { e2 } else { e }));
                end = e;
            }
            doc.objs.insert(num, Obj { v, stream });
            i = end.max(i);
        }
        doc
    }

    fn resolve<'b>(&'b self, p: &'b P) -> &'b P {
        let mut cur = p;
        for _ in 0..16 {
            match cur {
                P::Ref(n) => {
                    cur = match self.objs.get(n) {
                        Some(o) => &o.v,
                        None => match self.extra.get(n) {
                            Some(v) => v,
                            None => return &P::Null,
                        },
                    }
                }
                _ => return cur,
            }
        }
        &P::Null
    }

    fn obj(&self, n: u32) -> Option<&P> {
        self.objs.get(&n).map(|o| &o.v).or_else(|| self.extra.get(&n))
    }

    fn stream_of(&self, p: &P) -> Option<Vec<u8>> {
        let n = match p {
            P::Ref(n) => *n,
            _ => return None,
        };
        let o = self.objs.get(&n)?;
        let (s, e) = o.stream?;
        let mut data = self.d[s..e].to_vec();
        let filters: Vec<String> = match self.resolve(o.v.get("Filter").unwrap_or(&P::Null)) {
            P::Name(n) => vec![n.clone()],
            P::Arr(a) => a.iter().filter_map(|x| self.resolve(x).name().map(|s| s.to_string())).collect(),
            _ => vec![],
        };
        let parms: Vec<P> = match self.resolve(o.v.get("DecodeParms").unwrap_or(&P::Null)) {
            P::Arr(a) => a.iter().map(|x| self.resolve(x).clone()).collect(),
            P::Dict(d) => vec![P::Dict(d.clone())],
            _ => vec![],
        };
        for (i, f) in filters.iter().enumerate() {
            match f.as_str() {
                "FlateDecode" | "Fl" => {
                    data = flate(&data, self.cap);
                    if let Some(pm) = parms.get(i) {
                        let pred = pm.get("Predictor").and_then(|x| x.num()).unwrap_or(1.0) as i64;
                        if pred >= 10 {
                            let g = |k: &str, d: f64| pm.get(k).and_then(|x| x.num()).unwrap_or(d) as usize;
                            data = png_predict(&data, g("Columns", 1.0), g("Colors", 1.0), g("BitsPerComponent", 8.0));
                        }
                    }
                }
                "ASCIIHexDecode" | "AHx" => {
                    let hex: Vec<u8> = data.iter().copied().take_while(|&b| b != b'>').filter(|b| b.is_ascii_hexdigit()).collect();
                    data = hex.chunks(2).map(|h| u8::from_str_radix(&String::from_utf8_lossy(h), 16).unwrap_or(0)).collect();
                }
                "ASCII85Decode" | "A85" => data = ascii85(&data),
                _ => return None, // LZW, DCT, CCITT, JPX: not text streams we can read
            }
        }
        Some(data)
    }

    fn load_object_streams(&mut self) {
        let ids: Vec<u32> = self.objs.iter().filter(|(_, o)| o.v.get("Type").and_then(|t| t.name()) == Some("ObjStm")).map(|(k, _)| *k).collect();
        for id in ids {
            let (n, first) = {
                let o = &self.objs[&id].v;
                (o.get("N").and_then(|x| x.num()).unwrap_or(0.0) as usize, o.get("First").and_then(|x| x.num()).unwrap_or(0.0) as usize)
            };
            let data = match self.stream_of(&P::Ref(id)) {
                Some(d) => d,
                None => continue,
            };
            let mut ps = Parser::new(&data, 0);
            let mut pairs = Vec::new();
            for _ in 0..n.min(200_000) {
                let a = ps.parse().and_then(|x| x.num());
                let b = ps.parse().and_then(|x| x.num());
                match (a, b) {
                    (Some(a), Some(b)) => pairs.push((a as u32, b as usize)),
                    _ => break,
                }
            }
            for (num, off) in pairs {
                if self.objs.contains_key(&num) {
                    continue;
                }
                let mut p2 = Parser::new(&data, first + off);
                if let Some(v) = p2.parse() {
                    self.extra.insert(num, v);
                }
            }
        }
    }
}

// ---------------------------------------------------------------- fonts

struct Font {
    map: HashMap<u32, String>,
    width: usize,
    enc: Vec<Option<char>>,
    cid_without_map: bool,
}

fn cp1252(b: u8) -> char {
    const HI: [u32; 32] = [
        0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x8D, 0x017D, 0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x9D, 0x017E, 0x0178,
    ];
    if (0x80..0xA0).contains(&b) {
        char::from_u32(HI[(b - 0x80) as usize]).unwrap_or('?')
    } else {
        b as char
    }
}

fn glyph_char(n: &str) -> Option<char> {
    if n.chars().count() == 1 {
        return n.chars().next();
    }
    if let Some(h) = n.strip_prefix("uni").filter(|h| h.len() >= 4) {
        return u32::from_str_radix(&h[..4], 16).ok().and_then(char::from_u32);
    }
    Some(match n {
        "space" => ' ',
        "zero" => '0',
        "one" => '1',
        "two" => '2',
        "three" => '3',
        "four" => '4',
        "five" => '5',
        "six" => '6',
        "seven" => '7',
        "eight" => '8',
        "nine" => '9',
        "period" => '.',
        "comma" => ',',
        "colon" => ':',
        "semicolon" => ';',
        "hyphen" | "minus" => '-',
        "endash" => '–',
        "emdash" => '—',
        "quoteright" => '’',
        "quoteleft" => '‘',
        "quotedblleft" => '“',
        "quotedblright" => '”',
        "quotesingle" => '\'',
        "quotedbl" => '"',
        "parenleft" => '(',
        "parenright" => ')',
        "slash" => '/',
        "question" => '?',
        "exclam" => '!',
        "percent" => '%',
        "ampersand" => '&',
        "fi" => 'ﬁ',
        "fl" => 'ﬂ',
        "bullet" => '•',
        _ => return None,
    })
}

fn utf16be(b: &[u8]) -> String {
    let u: Vec<u16> = b.chunks(2).map(|c| if c.len() == 2 { u16::from_be_bytes([c[0], c[1]]) } else { c[0] as u16 }).collect();
    String::from_utf16_lossy(&u)
}

fn be(b: &[u8]) -> u32 {
    b.iter().fold(0u32, |a, &x| (a << 8) | x as u32)
}

fn parse_cmap(data: &[u8]) -> (HashMap<u32, String>, usize) {
    let mut map = HashMap::new();
    let mut width = 0usize;
    let text = data;
    let mut i = 0;
    while let Some(s) = find(text, b"begin", i) {
        let kind_end = text[s..].iter().position(|&b| is_ws(b)).map(|x| s + x).unwrap_or(text.len());
        let kind = std::str::from_utf8(&text[s..kind_end]).unwrap_or("");
        let end_tag: &[u8] = match kind {
            "beginbfchar" => b"endbfchar",
            "beginbfrange" => b"endbfrange",
            "begincodespacerange" => b"endcodespacerange",
            _ => {
                i = kind_end.max(s + 5);
                continue;
            }
        };
        let e = find(text, end_tag, kind_end).unwrap_or(text.len());
        let mut ps = Parser::new(&text[..e], kind_end);
        let mut toks = Vec::new();
        while let Some(t) = ps.parse() {
            if ps.p >= e {
                toks.push(t);
                break;
            }
            toks.push(t);
        }
        match kind {
            "begincodespacerange" => {
                if let Some(P::Str(b)) = toks.first() {
                    width = b.len();
                }
            }
            "beginbfchar" => {
                for c in toks.chunks(2) {
                    if let [P::Str(a), P::Str(b)] = c {
                        map.insert(be(a), utf16be(b));
                        if width == 0 {
                            width = a.len();
                        }
                    }
                }
            }
            _ => {
                for c in toks.chunks(3) {
                    if let [P::Str(lo), P::Str(hi), dst] = c {
                        if width == 0 {
                            width = lo.len();
                        }
                        let (lo, hi) = (be(lo), be(hi));
                        if hi < lo || hi - lo > 70_000 {
                            continue;
                        }
                        match dst {
                            P::Str(d) => {
                                let base = utf16be(d);
                                let mut chars: Vec<char> = base.chars().collect();
                                for k in 0..=(hi - lo) {
                                    map.insert(lo + k, chars.iter().collect());
                                    if let Some(l) = chars.last_mut() {
                                        *l = char::from_u32(*l as u32 + 1).unwrap_or(*l);
                                    }
                                }
                            }
                            P::Arr(a) => {
                                for (k, x) in a.iter().enumerate() {
                                    if let P::Str(d) = x {
                                        map.insert(lo + k as u32, utf16be(d));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        i = e + end_tag.len();
    }
    (map, width)
}

impl<'a> Doc<'a> {
    fn font(&self, fp: &P) -> Font {
        let f = self.resolve(fp);
        let sub = f.get("Subtype").and_then(|s| self.resolve(s).name()).unwrap_or("");
        let composite = sub == "Type0";
        let mut font = Font { map: HashMap::new(), width: if composite { 2 } else { 1 }, enc: vec![None; 256], cid_without_map: false };
        if let Some(tu) = f.get("ToUnicode") {
            if let Some(data) = self.stream_of(tu) {
                let (m, w) = parse_cmap(&data);
                if !m.is_empty() {
                    font.map = m;
                    if w > 0 {
                        font.width = w;
                    } else if composite {
                        font.width = 2;
                    }
                }
            }
        }
        if font.map.is_empty() && composite {
            font.cid_without_map = true;
        }
        // simple encodings
        let enc = f.get("Encoding").map(|e| self.resolve(e).clone());
        let base = match &enc {
            Some(P::Name(n)) => n.clone(),
            Some(d @ P::Dict(_)) => d.get("BaseEncoding").and_then(|b| b.name()).unwrap_or("StandardEncoding").to_string(),
            _ => "StandardEncoding".to_string(),
        };
        for b in 0..=255u8 {
            font.enc[b as usize] = Some(if base == "WinAnsiEncoding" { cp1252(b) } else { b as char });
        }
        if let Some(P::Dict(d)) = &enc {
            if let Some((_, P::Arr(diff))) = d.iter().find(|(k, _)| k == "Differences") {
                let mut code = 0usize;
                for x in diff {
                    match self.resolve(x) {
                        P::Num(n) => code = *n as usize,
                        P::Name(n) => {
                            if code < 256 {
                                font.enc[code] = glyph_char(n).or(font.enc[code]);
                            }
                            code += 1;
                        }
                        _ => {}
                    }
                }
            }
        }
        font
    }
}

impl Font {
    fn decode(&self, bytes: &[u8], out: &mut String, unmapped: &mut usize) {
        if !self.map.is_empty() {
            let w = self.width.max(1);
            for c in bytes.chunks(w) {
                if c.len() < w {
                    break;
                }
                match self.map.get(&be(c)) {
                    Some(s) => out.push_str(s),
                    None => {
                        if w == 1 && c[0] >= 32 {
                            out.push(c[0] as char);
                        } else {
                            *unmapped += 1;
                        }
                    }
                }
            }
        } else if self.cid_without_map {
            *unmapped += bytes.len() / 2;
        } else {
            for &b in bytes {
                if let Some(c) = self.enc[b as usize] {
                    if c != '\0' {
                        out.push(c);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- content streams

struct Lines {
    done: Vec<(String, bool)>, // (line, paragraph break before it)
    cur: String,
    last_y: Option<f64>,
    gap_before_cur: bool,
    unmapped: usize,
}

impl Lines {
    fn flush(&mut self) {
        let t = self.cur.trim_end().to_string();
        if !t.is_empty() {
            self.done.push((t, self.gap_before_cur));
        }
        self.cur.clear();
        self.gap_before_cur = false;
    }
    fn at(&mut self, y: f64, size: f64) {
        if let Some(ly) = self.last_y {
            let dy = (ly - y).abs();
            if dy > 0.3 * size {
                self.flush();
                self.gap_before_cur = dy > 1.75 * size;
            } else if !self.cur.is_empty() && !self.cur.ends_with(' ') && dy <= 0.3 * size {
                // same line, new positioning: a word gap
            }
        }
        self.last_y = Some(y);
    }
}

struct Gfx {
    font: Option<usize>,
    size: f64,
    ly: f64,
    scale: f64,
    leading: f64,
}

impl<'a> Doc<'a> {
    fn run(&self, data: &[u8], res: &P, fonts: &mut HashMap<String, Font>, out: &mut Lines, depth: usize, seen: &mut HashSet<u32>) {
        if depth > 4 {
            return;
        }
        let font_dict = res.get("Font").map(|f| self.resolve(f).clone());
        let mut ps = Parser::new(data, 0);
        let mut stack: Vec<P> = Vec::new();
        let mut g = Gfx { font: None, size: 10.0, ly: 0.0, scale: 1.0, leading: 0.0 };
        let mut cur_font: Option<String> = None;
        let mut ops = 0usize;
        let mut pending_space = false;
        while let Some(t) = ps.parse() {
            ops += 1;
            if ops > 5_000_000 {
                break;
            }
            let op = match t {
                P::Op(o) => o,
                other => {
                    stack.push(other);
                    if stack.len() > 512 {
                        stack.drain(..256);
                    }
                    continue;
                }
            };
            let nums: Vec<f64> = stack.iter().filter_map(|x| x.num()).collect();
            let eff = |g: &Gfx| (g.size * g.scale.abs()).max(1.0);
            match op.as_str() {
                "Tf" => {
                    if let (Some(P::Name(n)), Some(sz)) = (stack.get(stack.len().wrapping_sub(2)), stack.last().and_then(|x| x.num())) {
                        g.size = if sz.abs() < 0.01 { 10.0 } else { sz.abs() };
                        let key = n.clone();
                        if !fonts.contains_key(&key) {
                            if let Some(fd) = &font_dict {
                                if let Some(fp) = fd.get(&key) {
                                    let f = self.font(fp);
                                    fonts.insert(key.clone(), f);
                                }
                            }
                        }
                        cur_font = Some(key);
                        g.font = Some(0);
                    }
                }
                "Td" | "TD" => {
                    if nums.len() >= 2 {
                        let (tx, ty) = (nums[nums.len() - 2], nums[nums.len() - 1]);
                        g.ly += ty * g.scale;
                        if op == "TD" {
                            g.leading = -ty;
                        }
                        if ty.abs() * g.scale.abs() <= 0.3 * eff(&g) && tx > 0.0 {
                            pending_space = true;
                        }
                    }
                }
                "Tm" => {
                    if nums.len() >= 6 {
                        g.ly = nums[5];
                        g.scale = if nums[3].abs() > 1e-6 { nums[3].abs() } else { nums[0].abs().max(1e-6) };
                        pending_space = true;
                    }
                }
                "TL" => {
                    if let Some(l) = nums.last() {
                        g.leading = *l;
                    }
                }
                "T*" => g.ly -= g.leading * g.scale,
                "BT" => {
                    g.ly = 0.0;
                    g.scale = 1.0;
                    pending_space = true;
                }
                "Tj" | "'" | "\"" | "TJ" => {
                    if op == "'" || op == "\"" {
                        g.ly -= g.leading * g.scale;
                    }
                    let size = eff(&g);
                    out.at(g.ly, size);
                    if pending_space && !out.cur.is_empty() && !out.cur.ends_with(' ') {
                        out.cur.push(' ');
                    }
                    pending_space = false;
                    let font = cur_font.as_ref().and_then(|k| fonts.get(k));
                    let mut emit = |bytes: &[u8], out: &mut Lines| {
                        let mut s = String::new();
                        match font {
                            Some(f) => f.decode(bytes, &mut s, &mut out.unmapped),
                            None => s.extend(bytes.iter().filter(|&&b| b >= 32).map(|&b| b as char)),
                        }
                        out.cur.push_str(&s);
                    };
                    match stack.last() {
                        Some(P::Str(s)) => emit(s, out),
                        Some(P::Arr(a)) => {
                            for x in a {
                                match x {
                                    P::Str(s) => emit(s, out),
                                    P::Num(n) if *n < -180.0 && !out.cur.ends_with(' ') && !out.cur.is_empty() => out.cur.push(' '),
                                    _ => {}
                                }
                            }
                        }
                        _ => {}
                    }
                }
                "Do" => {
                    if let (Some(P::Name(n)), Some(xo)) = (stack.last(), res.get("XObject").map(|x| self.resolve(x))) {
                        if let Some(P::Ref(id)) = xo.get(n) {
                            let id = *id;
                            if seen.insert(id) {
                                if let Some(o) = self.objs.get(&id) {
                                    if o.v.get("Subtype").and_then(|s| s.name()) == Some("Form") {
                                        if let Some(sd) = self.stream_of(&P::Ref(id)) {
                                            let r2 = o.v.get("Resources").map(|r| self.resolve(r).clone()).unwrap_or_else(|| res.clone());
                                            self.run(&sd, &r2, fonts, out, depth + 1, seen);
                                        }
                                    }
                                }
                                seen.remove(&id);
                            }
                        }
                    }
                }
                _ => {}
            }
            stack.clear();
        }
    }
}

// ---------------------------------------------------------------- pages

fn page_list(doc: &Doc) -> Vec<(P, P)> {
    // (page dict, inherited resources)
    let mut root_pages: Option<P> = None;
    for o in doc.objs.values().chain(std::iter::empty()) {
        if o.v.get("Type").and_then(|t| t.name()) == Some("Catalog") {
            if let Some(p) = o.v.get("Pages") {
                root_pages = Some(p.clone());
            }
        }
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    fn walk(doc: &Doc, node: &P, res: &P, out: &mut Vec<(P, P)>, seen: &mut HashSet<u32>, depth: usize) {
        if depth > 40 || out.len() > 100_000 {
            return;
        }
        if let P::Ref(n) = node {
            if !seen.insert(*n) {
                return;
            }
        }
        let n = doc.resolve(node);
        let res2 = n.get("Resources").map(|r| doc.resolve(r).clone()).unwrap_or_else(|| res.clone());
        match n.get("Kids").map(|k| doc.resolve(k)) {
            Some(P::Arr(kids)) => {
                for k in kids {
                    walk(doc, k, &res2, out, seen, depth + 1);
                }
            }
            _ => {
                if n.get("Contents").is_some() || n.get("Type").and_then(|t| t.name()) == Some("Page") {
                    out.push((n.clone(), res2));
                }
            }
        }
    }
    if let Some(rp) = root_pages {
        walk(doc, &rp, &P::Null, &mut out, &mut seen, 0);
    }
    if out.is_empty() {
        let mut ids: Vec<&u32> = doc.objs.iter().filter(|(_, o)| o.v.get("Type").and_then(|t| t.name()) == Some("Page")).map(|(k, _)| k).collect();
        ids.sort();
        for id in ids {
            let v = &doc.objs[id].v;
            out.push((v.clone(), v.get("Resources").map(|r| doc.resolve(r).clone()).unwrap_or(P::Null)));
        }
    }
    out
}

fn join_paragraphs(lines: &[(String, bool)]) -> String {
    let mut paras: Vec<String> = Vec::new();
    let mut cur = String::new();
    for (i, (l, gap)) in lines.iter().enumerate() {
        if *gap && i > 0 && !cur.is_empty() {
            paras.push(std::mem::take(&mut cur));
        }
        if cur.is_empty() {
            cur.push_str(l);
        } else if cur.ends_with('-') && l.chars().next().map_or(false, |c| c.is_lowercase()) {
            cur.pop();
            cur.push_str(l);
        } else {
            cur.push(' ');
            cur.push_str(l);
        }
    }
    if !cur.is_empty() {
        paras.push(cur);
    }
    paras.join("\n\n")
}

pub struct PdfText {
    pub text: String,
    pub pages: usize,
    pub warnings: Vec<String>,
}

pub fn extract_bytes(d: &[u8], max_stream: usize) -> Result<PdfText, String> {
    if !d.windows(5).take(1024).any(|w| w == b"%PDF-") {
        return Err("not a PDF file (no %PDF header)".into());
    }
    let mut doc = Doc::scan(d, max_stream);
    if doc.objs.is_empty() {
        return Err("no PDF objects found; the file may be damaged".into());
    }
    doc.load_object_streams();
    // encrypted?
    let enc = doc.objs.values().any(|o| o.v.get("Type").and_then(|t| t.name()) == Some("XRef") && o.v.get("Encrypt").is_some())
        || find(d, b"trailer", 0).map_or(false, |t| {
            let mut ps = Parser::new(d, t + 7);
            ps.parse().map_or(false, |v| v.get("Encrypt").is_some())
        })
        || {
            let mut found = false;
            let mut from = 0;
            while let Some(t) = find(d, b"trailer", from) {
                from = t + 7;
                let mut ps = Parser::new(d, t + 7);
                if ps.parse().map_or(false, |v| v.get("Encrypt").is_some()) {
                    found = true;
                    break;
                }
            }
            found
        };
    if enc {
        return Err("the PDF is encrypted (password protected); remove the protection first".into());
    }
    let pages = page_list(&doc);
    if pages.is_empty() {
        return Err("no pages found in the PDF".into());
    }
    let mut warnings = Vec::new();
    let mut fonts: HashMap<String, Font> = HashMap::new();
    let mut all: Vec<String> = Vec::new();
    let mut unmapped_total = 0usize;
    let mut empty_pages = 0usize;
    for (pi, (page, res)) in pages.iter().enumerate() {
        fonts.clear(); // font resource names are only meaningful inside one page's resources
        let mut out = Lines { done: Vec::new(), cur: String::new(), last_y: None, gap_before_cur: false, unmapped: 0 };
        let mut streams: Vec<P> = Vec::new();
        match page.get("Contents").map(|c| doc.resolve_keep(c)) {
            Some(P::Arr(a)) => streams.extend(a.iter().cloned()),
            Some(r @ P::Ref(_)) => streams.push(r.clone()),
            _ => {}
        }
        let mut data = Vec::new();
        for s in &streams {
            if let Some(b) = doc.stream_of(s) {
                data.extend_from_slice(&b);
                data.push(b'\n');
            }
        }
        let mut seen = HashSet::new();
        doc.run(&data, res, &mut fonts, &mut out, 0, &mut seen);
        out.flush();
        unmapped_total += out.unmapped;
        if out.done.is_empty() {
            empty_pages += 1;
            continue;
        }
        let _ = pi;
        all.push(join_paragraphs(&out.done));
    }
    if unmapped_total > 0 {
        warnings.push(format!("{} characters could not be mapped to Unicode (fonts without a ToUnicode table) and were skipped", unmapped_total));
    }
    if empty_pages > 0 {
        warnings.push(format!("{} of {} pages had no extractable text (scanned images need OCR, which is not supported)", empty_pages, pages.len()));
    }
    if all.is_empty() {
        return Err("no extractable text: the PDF looks like scanned images (OCR is not supported)".into());
    }
    Ok(PdfText { text: all.join("\n\n"), pages: pages.len(), warnings })
}

impl<'a> Doc<'a> {
    /// Like resolve, but leaves a reference to a stream object as a reference (needed to read its bytes).
    fn resolve_keep<'b>(&'b self, p: &'b P) -> &'b P {
        if let P::Ref(n) = p {
            if let Some(o) = self.objs.get(n) {
                if o.stream.is_none() {
                    return self.resolve(p);
                }
            } else {
                return self.resolve(p);
            }
        }
        p
    }
}

pub fn extract(path: &std::path::Path, cap: usize) -> Result<PdfText, String> {
    let len = std::fs::metadata(path).map_err(|e| format!("cannot open the file: {}", e))?.len();
    if len as usize > cap {
        return Err(format!("the PDF is {} MiB, above the {} MiB limit", len >> 20, cap >> 20));
    }
    let d = std::fs::read(path).map_err(|e| format!("cannot read the file: {}", e))?;
    extract_bytes(&d, 256 << 20)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sample_pdf() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/sample.pdf");
        let r = extract(&p, 1 << 28).unwrap();
        assert_eq!(r.pages, 2);
        assert!(r.text.contains("MNX DataForge test document"), "{}", r.text);
        assert!(r.text.contains("hyphenated"), "{}", r.text);
        assert!(r.text.contains("café"), "{}", r.text);
        assert!(r.text.contains("Page two content here."));
        assert!(r.text.contains("\n\nSecond paragraph"), "{}", r.text);
    }
    #[test]
    fn rejects_garbage() {
        assert!(extract_bytes(b"hello", 1 << 20).is_err());
    }
}
