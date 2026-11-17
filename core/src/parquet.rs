//! Parquet reader (flat schemas): row groups are decoded one at a time with a memory cap.
//! Codecs: uncompressed, Snappy, GZIP, ZSTD, LZ4_RAW. Encodings: PLAIN and dictionary (PLAIN_DICTIONARY /
//! RLE_DICTIONARY) with RLE/bit-packed definition levels. Nested columns and other encodings are reported and
//! skipped per column; the remaining columns are still read.

use crate::inflate::Gunzip;
use crate::json::Value;
use crate::reader::{Batch, Limits, Records};
use crate::zstd::Zstd;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

fn err<T>(m: impl Into<String>) -> Result<T, String> {
    Err(m.into())
}

// ---------------------------------------------------------------- Thrift compact (generic tree)

#[derive(Debug, Clone)]
enum T {
    Bool(bool),
    I(i64),
    Bin(Vec<u8>),
    List(Vec<T>),
    Struct(Vec<(i16, T)>),
    Other,
}

impl T {
    fn field(&self, id: i16) -> Option<&T> {
        match self {
            T::Struct(f) => f.iter().find(|(i, _)| *i == id).map(|(_, v)| v),
            _ => None,
        }
    }
    fn int(&self, id: i16) -> Option<i64> {
        match self.field(id) {
            Some(T::I(v)) => Some(*v),
            _ => None,
        }
    }
    fn list(&self, id: i16) -> &[T] {
        match self.field(id) {
            Some(T::List(l)) => l,
            _ => &[],
        }
    }
    fn string(&self, id: i16) -> Option<String> {
        match self.field(id) {
            Some(T::Bin(b)) => Some(String::from_utf8_lossy(b).to_string()),
            _ => None,
        }
    }
}

struct Th<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Th<'a> {
    fn byte(&mut self) -> Result<u8, String> {
        let b = *self.d.get(self.p).ok_or("truncated metadata")?;
        self.p += 1;
        Ok(b)
    }
    fn varint(&mut self) -> Result<u64, String> {
        let mut v = 0u64;
        let mut s = 0;
        loop {
            let b = self.byte()?;
            v |= ((b & 0x7f) as u64) << s;
            if b & 0x80 == 0 {
                return Ok(v);
            }
            s += 7;
            if s > 63 {
                return err("bad varint");
            }
        }
    }
    fn zig(&mut self) -> Result<i64, String> {
        let v = self.varint()?;
        Ok((v >> 1) as i64 ^ -((v & 1) as i64))
    }
    fn value(&mut self, t: u8, depth: usize) -> Result<T, String> {
        if depth > 32 {
            return err("metadata nesting too deep");
        }
        Ok(match t {
            1 => T::Bool(true),
            2 => T::Bool(false),
            3 => T::I(self.byte()? as i8 as i64),
            4 | 5 | 6 => T::I(self.zig()?),
            7 => {
                self.p += 8;
                if self.p > self.d.len() {
                    return err("truncated metadata");
                }
                T::Other
            }
            8 => {
                let n = self.varint()? as usize;
                if n > self.d.len() - self.p {
                    return err("truncated metadata");
                }
                let v = self.d[self.p..self.p + n].to_vec();
                self.p += n;
                T::Bin(v)
            }
            9 | 10 => {
                let h = self.byte()?;
                let mut n = (h >> 4) as usize;
                if n == 15 {
                    n = self.varint()? as usize;
                }
                let et = h & 15;
                if n > self.d.len() {
                    return err("bad list size");
                }
                let mut out = Vec::with_capacity(n);
                for _ in 0..n {
                    out.push(if et == 1 || et == 2 { T::Bool(self.byte()? == 1) } else { self.value(et, depth + 1)? });
                }
                T::List(out)
            }
            11 => {
                let n = self.varint()? as usize;
                if n > 0 {
                    let kv = self.byte()?;
                    for _ in 0..n {
                        self.value(kv >> 4, depth + 1)?;
                        self.value(kv & 15, depth + 1)?;
                    }
                }
                T::Other
            }
            12 => self.strukt(depth + 1)?,
            _ => return err("unknown thrift type"),
        })
    }
    fn strukt(&mut self, depth: usize) -> Result<T, String> {
        let mut fields = Vec::new();
        let mut last: i16 = 0;
        loop {
            let b = self.byte()?;
            if b == 0 {
                return Ok(T::Struct(fields));
            }
            let t = b & 15;
            let delta = (b >> 4) as i16;
            let id = if delta == 0 { self.zig()? as i16 } else { last + delta };
            last = id;
            fields.push((id, self.value(t, depth)?));
        }
    }
}

// ---------------------------------------------------------------- decompression

fn snappy(src: &[u8], cap: usize) -> Result<Vec<u8>, String> {
    let (mut n, mut shift, mut p) = (0usize, 0, 0);
    loop {
        let b = *src.get(p).ok_or("truncated snappy data")?;
        p += 1;
        n |= ((b & 0x7f) as usize) << shift;
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 35 {
            return err("bad snappy length");
        }
    }
    if n > cap {
        return err("decompressed page is larger than the limit");
    }
    let mut out: Vec<u8> = Vec::with_capacity(n);
    while p < src.len() {
        let tag = src[p];
        p += 1;
        match tag & 3 {
            0 => {
                let mut len = (tag >> 2) as usize;
                if len >= 60 {
                    let extra = len - 59;
                    if p + extra > src.len() {
                        return err("truncated snappy literal");
                    }
                    len = 0;
                    for i in 0..extra {
                        len |= (src[p + i] as usize) << (8 * i);
                    }
                    p += extra;
                }
                len += 1;
                if p + len > src.len() || out.len() + len > n {
                    return err("corrupt snappy literal");
                }
                out.extend_from_slice(&src[p..p + len]);
                p += len;
            }
            t => {
                let (len, off) = match t {
                    1 => {
                        if p >= src.len() {
                            return err("truncated snappy copy");
                        }
                        let o = (((tag >> 5) as usize) << 8) | src[p] as usize;
                        p += 1;
                        (((tag >> 2) & 7) as usize + 4, o)
                    }
                    2 => {
                        if p + 2 > src.len() {
                            return err("truncated snappy copy");
                        }
                        let o = src[p] as usize | (src[p + 1] as usize) << 8;
                        p += 2;
                        ((tag >> 2) as usize + 1, o)
                    }
                    _ => {
                        if p + 4 > src.len() {
                            return err("truncated snappy copy");
                        }
                        let o = u32::from_le_bytes([src[p], src[p + 1], src[p + 2], src[p + 3]]) as usize;
                        p += 4;
                        ((tag >> 2) as usize + 1, o)
                    }
                };
                if off == 0 || off > out.len() || out.len() + len > n {
                    return err("corrupt snappy copy");
                }
                for _ in 0..len {
                    let b = out[out.len() - off];
                    out.push(b);
                }
            }
        }
    }
    if out.len() != n {
        return err("snappy length mismatch");
    }
    Ok(out)
}

fn lz4_raw(src: &[u8], size: usize) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::with_capacity(size);
    let mut p = 0;
    while p < src.len() {
        let tok = src[p];
        p += 1;
        let mut lit = (tok >> 4) as usize;
        if lit == 15 {
            loop {
                let b = *src.get(p).ok_or("truncated lz4")?;
                p += 1;
                lit += b as usize;
                if b != 255 {
                    break;
                }
            }
        }
        if p + lit > src.len() || out.len() + lit > size {
            return err("corrupt lz4 literal");
        }
        out.extend_from_slice(&src[p..p + lit]);
        p += lit;
        if p >= src.len() {
            break;
        }
        if p + 2 > src.len() {
            return err("truncated lz4 offset");
        }
        let off = src[p] as usize | (src[p + 1] as usize) << 8;
        p += 2;
        let mut ml = (tok & 15) as usize;
        if ml == 15 {
            loop {
                let b = *src.get(p).ok_or("truncated lz4")?;
                p += 1;
                ml += b as usize;
                if b != 255 {
                    break;
                }
            }
        }
        ml += 4;
        if off == 0 || off > out.len() || out.len() + ml > size {
            return err("corrupt lz4 match");
        }
        for _ in 0..ml {
            let b = out[out.len() - off];
            out.push(b);
        }
    }
    if out.len() != size {
        return err("lz4 length mismatch");
    }
    Ok(out)
}

fn decompress(codec: i64, src: &[u8], usize_: usize, cap: usize) -> Result<Vec<u8>, String> {
    if usize_ > cap {
        return err("a page is larger than the memory limit");
    }
    match codec {
        0 => Ok(src.to_vec()),
        1 => snappy(src, cap),
        2 => {
            let mut out = Vec::with_capacity(usize_);
            Gunzip::new(src).take(cap as u64 + 1).read_to_end(&mut out).map_err(|e| e.to_string())?;
            Ok(out)
        }
        6 => {
            let mut out = Vec::with_capacity(usize_);
            Zstd::new(src, 64 << 20).take(cap as u64 + 1).read_to_end(&mut out).map_err(|e| e.to_string())?;
            Ok(out)
        }
        7 => lz4_raw(src, usize_),
        c => err(format!("unsupported Parquet compression codec {} (supported: none, snappy, gzip, zstd, lz4_raw)", c)),
    }
}

// ---------------------------------------------------------------- levels and values

struct Hybrid<'a> {
    d: &'a [u8],
    p: usize,
    width: u32,
    run_left: usize,
    run_val: u32,
    packed: bool,
    bit_pos: usize,
}

impl<'a> Hybrid<'a> {
    fn new(d: &'a [u8], width: u32) -> Self {
        Hybrid { d, p: 0, width, run_left: 0, run_val: 0, packed: false, bit_pos: 0 }
    }
    fn next(&mut self) -> Result<u32, String> {
        if self.run_left == 0 {
            let mut v = 0usize;
            let mut s = 0;
            loop {
                let b = *self.d.get(self.p).ok_or("truncated RLE data")?;
                self.p += 1;
                v |= ((b & 0x7f) as usize) << s;
                if b & 0x80 == 0 {
                    break;
                }
                s += 7;
                if s > 35 {
                    return err("bad RLE header");
                }
            }
            if v & 1 == 0 {
                self.run_left = v >> 1;
                let nb = (self.width as usize).div_ceil(8);
                if self.p + nb > self.d.len() {
                    return err("truncated RLE run");
                }
                let mut x = 0u32;
                for i in 0..nb {
                    x |= (self.d[self.p + i] as u32) << (8 * i);
                }
                self.p += nb;
                self.run_val = x;
                self.packed = false;
            } else {
                self.run_left = (v >> 1) * 8;
                self.packed = true;
                self.bit_pos = self.p * 8;
                let bytes = (v >> 1) * self.width as usize;
                if self.p + bytes > self.d.len() {
                    return err("truncated bit-packed run");
                }
                self.p += bytes;
            }
            if self.run_left == 0 {
                return self.next();
            }
        }
        self.run_left -= 1;
        if !self.packed {
            return Ok(self.run_val);
        }
        let mut x = 0u32;
        for i in 0..self.width as usize {
            let bp = self.bit_pos + i;
            if (self.d[bp / 8] >> (bp % 8)) & 1 == 1 {
                x |= 1 << i;
            }
        }
        self.bit_pos += self.width as usize;
        Ok(x)
    }
}

#[derive(Clone)]
struct Col {
    name: String,
    ptype: i64,
    optional: bool,
    utf8: bool,
    type_len: usize,
}

fn read_plain(col: &Col, d: &[u8], n: usize) -> Result<Vec<Value>, String> {
    let mut out = Vec::with_capacity(n);
    let mut p = 0usize;
    for _ in 0..n {
        match col.ptype {
            0 => {
                let i = out.len();
                let byte = *d.get(i / 8).ok_or("truncated boolean data")?;
                out.push(Value::Bool((byte >> (i % 8)) & 1 == 1));
            }
            1 => {
                let b = d.get(p..p + 4).ok_or("truncated int32 data")?;
                out.push(Value::int(i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as i64));
                p += 4;
            }
            2 => {
                let b = d.get(p..p + 8).ok_or("truncated int64 data")?;
                out.push(Value::int(i64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])));
                p += 8;
            }
            4 => {
                let b = d.get(p..p + 4).ok_or("truncated float data")?;
                out.push(Value::float(f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64));
                p += 4;
            }
            5 => {
                let b = d.get(p..p + 8).ok_or("truncated double data")?;
                out.push(Value::float(f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])));
                p += 8;
            }
            6 => {
                let lb = d.get(p..p + 4).ok_or("truncated string data")?;
                let len = u32::from_le_bytes([lb[0], lb[1], lb[2], lb[3]]) as usize;
                p += 4;
                let b = d.get(p..p + len).ok_or("truncated string data")?;
                p += len;
                match std::str::from_utf8(b) {
                    Ok(s) => out.push(Value::str(s)),
                    Err(_) => out.push(if col.utf8 { Value::str(String::from_utf8_lossy(b).to_string()) } else { Value::str(b.iter().map(|x| format!("{:02x}", x)).collect::<String>()) }),
                }
            }
            7 => {
                let b = d.get(p..p + col.type_len).ok_or("truncated fixed-length data")?;
                p += col.type_len;
                out.push(Value::str(b.iter().map(|x| format!("{:02x}", x)).collect::<String>()));
            }
            3 => {
                let b = d.get(p..p + 12).ok_or("truncated int96 data")?;
                p += 12;
                out.push(Value::str(b.iter().map(|x| format!("{:02x}", x)).collect::<String>()));
            }
            t => return err(format!("unsupported Parquet physical type {}", t)),
        }
    }
    Ok(out)
}

struct Chunk {
    col: usize,
    codec: i64,
    num_values: i64,
    start: u64,
    size: u64,
}

struct Group {
    rows: i64,
    chunks: Vec<Chunk>,
}

pub struct ParquetRecords {
    f: std::fs::File,
    name: String,
    cols: Vec<Col>,
    skipped_cols: Vec<(String, String)>,
    groups: Vec<Group>,
    gi: usize,
    decoded: Vec<(usize, Vec<Value>)>, // (column index, values for the current row group)
    rows_in_group: usize,
    row: usize,
    cap: usize,
    announced: bool,
}

pub fn open(path: &Path, name: String, lim: &Limits) -> Result<Box<dyn Records>, String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let len = f.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
    if len < 12 {
        return err("file is too small to be Parquet");
    }
    let mut tail = [0u8; 8];
    f.seek(SeekFrom::Start(len - 8)).map_err(|e| e.to_string())?;
    f.read_exact(&mut tail).map_err(|e| e.to_string())?;
    if &tail[4..] != b"PAR1" {
        return err("not a complete Parquet file (footer magic missing; the file may be truncated)");
    }
    let mlen = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]) as u64;
    if mlen + 12 > len || mlen > 256 << 20 {
        return err("corrupt Parquet footer length");
    }
    f.seek(SeekFrom::Start(len - 8 - mlen)).map_err(|e| e.to_string())?;
    let mut meta = vec![0u8; mlen as usize];
    f.read_exact(&mut meta).map_err(|e| e.to_string())?;
    let root = Th { d: &meta, p: 0 }.strukt(0)?;
    let schema = root.list(2);
    if schema.is_empty() {
        return err("Parquet schema is empty");
    }
    let mut cols: Vec<Col> = Vec::new();
    let mut skipped = Vec::new();
    let mut leaf_ix: Vec<Option<usize>> = Vec::new(); // schema leaf order -> usable column index
    let mut i = 1usize;
    let top_children = schema[0].int(5).unwrap_or(0) as usize;
    for _ in 0..top_children {
        let Some(el) = schema.get(i) else { return err("schema is inconsistent") };
        let nm = el.string(4).unwrap_or_default();
        let nchild = el.int(5).unwrap_or(0);
        if nchild > 0 {
            // skip the whole nested subtree, counting its leaves
            let mut need = nchild as usize;
            let mut leaves = 0usize;
            i += 1;
            while need > 0 && i < schema.len() {
                let c = schema[i].int(5).unwrap_or(0);
                if c > 0 {
                    need += c as usize;
                } else {
                    leaves += 1;
                }
                need -= 1;
                i += 1;
            }
            for _ in 0..leaves {
                leaf_ix.push(None);
            }
            skipped.push((nm, "nested/list column is not supported and was skipped".to_string()));
            continue;
        }
        let rep = el.int(3).unwrap_or(0);
        if rep == 2 {
            leaf_ix.push(None);
            skipped.push((nm, "repeated column is not supported and was skipped".to_string()));
            i += 1;
            continue;
        }
        let conv = el.int(6);
        let logical_string = matches!(el.field(10), Some(T::Struct(f)) if f.iter().any(|(id, _)| *id == 1));
        cols.push(Col { name: nm, ptype: el.int(1).unwrap_or(-1), optional: rep == 1, utf8: conv == Some(0) || logical_string, type_len: el.int(2).unwrap_or(0).max(0) as usize });
        leaf_ix.push(Some(cols.len() - 1));
        i += 1;
    }
    let mut groups = Vec::new();
    for rg in root.list(4) {
        let mut chunks = Vec::new();
        for (leaf, cc) in rg.list(1).iter().enumerate() {
            let Some(Some(ci)) = leaf_ix.get(leaf).copied() else { continue };
            let Some(md) = cc.field(3) else { continue };
            let dict = md.int(11).filter(|&o| o > 0);
            let data = md.int(9).unwrap_or(0);
            let start = dict.unwrap_or(data).max(0) as u64;
            chunks.push(Chunk { col: ci, codec: md.int(4).unwrap_or(0), num_values: md.int(5).unwrap_or(0), start, size: md.int(7).unwrap_or(0).max(0) as u64 });
        }
        groups.push(Group { rows: rg.int(3).unwrap_or(0), chunks });
    }
    Ok(Box::new(ParquetRecords { f, name, cols, skipped_cols: skipped, groups, gi: 0, decoded: Vec::new(), rows_in_group: 0, row: 0, cap: lim.whole_file_cap, announced: false }))
}

impl ParquetRecords {
    fn decode_chunk(&mut self, ch: &Chunk) -> Result<Vec<Value>, String> {
        let col = self.cols[ch.col].clone();
        if ch.size as usize > self.cap {
            return err(format!("column chunk is {} MiB, above the {} MiB limit", ch.size >> 20, self.cap >> 20));
        }
        self.f.seek(SeekFrom::Start(ch.start)).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; ch.size as usize];
        self.f.read_exact(&mut buf).map_err(|_| "column data is truncated".to_string())?;
        let mut out: Vec<Value> = Vec::with_capacity(ch.num_values.max(0) as usize);
        let mut dict: Option<Vec<Value>> = None;
        let mut p = 0usize;
        while p < buf.len() && (out.len() as i64) < ch.num_values {
            let mut th = Th { d: &buf, p };
            let ph = th.strukt(0)?;
            p = th.p;
            let ptype = ph.int(1).unwrap_or(-1);
            let usz = ph.int(2).unwrap_or(0).max(0) as usize;
            let csz = ph.int(3).unwrap_or(0).max(0) as usize;
            let body = buf.get(p..p + csz).ok_or("page extends past the column data")?;
            p += csz;
            match ptype {
                2 => {
                    let h = ph.field(7).ok_or("dictionary page header missing")?;
                    let n = h.int(1).unwrap_or(0).max(0) as usize;
                    let data = decompress(ch.codec, body, usz, self.cap)?;
                    dict = Some(read_plain(&col, &data, n)?);
                }
                0 | 3 => {
                    let v2 = ptype == 3;
                    let h = ph.field(if v2 { 8 } else { 5 }).ok_or("data page header missing")?;
                    let n = h.int(1).unwrap_or(0).max(0) as usize;
                    let enc = h.int(if v2 { 4 } else { 2 }).unwrap_or(0);
                    let (levels, values): (Vec<u8>, Vec<u8>);
                    let max_def: u32 = u32::from(col.optional);
                    let mut def_block: Vec<u8> = Vec::new();
                    if v2 {
                        let dl = h.int(5).unwrap_or(0).max(0) as usize;
                        let rl = h.int(6).unwrap_or(0).max(0) as usize;
                        if rl > 0 {
                            return err("repeated data is not supported");
                        }
                        if dl > body.len() {
                            return err("corrupt page (levels longer than page)");
                        }
                        levels = body[..dl].to_vec();
                        let compressed = !matches!(h.field(7), Some(T::Bool(false)));
                        values = if compressed { decompress(ch.codec, &body[dl..], usz.saturating_sub(dl), self.cap)? } else { body[dl..].to_vec() };
                        def_block = levels;
                    } else {
                        let data = decompress(ch.codec, body, usz, self.cap)?;
                        let mut off = 0usize;
                        if max_def > 0 {
                            let l = data.get(0..4).ok_or("truncated levels")?;
                            let len = u32::from_le_bytes([l[0], l[1], l[2], l[3]]) as usize;
                            def_block = data.get(4..4 + len).ok_or("truncated levels")?.to_vec();
                            off = 4 + len;
                        }
                        values = data[off..].to_vec();
                    }
                    let mut defs: Vec<bool> = Vec::with_capacity(n);
                    if max_def > 0 {
                        let mut hy = Hybrid::new(&def_block, 1);
                        for _ in 0..n {
                            defs.push(hy.next()? == 1);
                        }
                    } else {
                        defs.resize(n, true);
                    }
                    let present = defs.iter().filter(|x| **x).count();
                    let vals: Vec<Value> = match enc {
                        0 => read_plain(&col, &values, present)?,
                        2 | 8 => {
                            let d = dict.as_ref().ok_or("dictionary-encoded page without a dictionary")?;
                            let width = *values.first().ok_or("truncated dictionary indices")? as u32;
                            if width > 32 {
                                return err("bad dictionary index width");
                            }
                            let mut hy = Hybrid::new(&values[1..], width);
                            let mut v = Vec::with_capacity(present);
                            for _ in 0..present {
                                let ix = hy.next()? as usize;
                                v.push(d.get(ix).cloned().ok_or("dictionary index out of range")?);
                            }
                            v
                        }
                        e => return err(format!("unsupported Parquet encoding {} (supported: PLAIN, dictionary)", e)),
                    };
                    let mut it = vals.into_iter();
                    for d in defs {
                        out.push(if d { it.next().unwrap_or(Value::Null) } else { Value::Null });
                    }
                }
                _ => {}
            }
        }
        Ok(out)
    }

    fn load_group(&mut self, b: &mut Batch) {
        self.decoded.clear();
        self.row = 0;
        let g = self.gi;
        self.rows_in_group = 0;
        let chunks: Vec<(usize, i64, i64, u64, u64)> = self.groups[g].chunks.iter().map(|c| (c.col, c.codec, c.num_values, c.start, c.size)).collect();
        let mut expected = self.groups[g].rows.max(0) as usize;
        for (col, codec, nv, start, size) in chunks {
            let ch = Chunk { col, codec, num_values: nv, start, size };
            match self.decode_chunk(&ch) {
                Ok(v) => {
                    if expected == 0 {
                        expected = v.len();
                    }
                    self.decoded.push((col, v));
                }
                Err(e) => b.err(&format!("{}#rowgroup{}.{}", self.name, g, self.cols[col].name), format!("{} (column skipped for this row group)", e)),
            }
        }
        self.rows_in_group = self.decoded.iter().map(|(_, v)| v.len()).max().unwrap_or(0).min(expected.max(1) * 2);
    }
}

impl Records for ParquetRecords {
    fn next(&mut self, max_records: usize, max_bytes: usize, b: &mut Batch) -> bool {
        if !self.announced {
            self.announced = true;
            for (n, m) in &self.skipped_cols {
                b.err(&format!("{}.{}", self.name, n), m.clone());
            }
        }
        loop {
            if b.records.len() >= max_records || b.bytes >= max_bytes {
                return false;
            }
            if self.row >= self.rows_in_group {
                if self.gi >= self.groups.len() {
                    return true;
                }
                if !self.decoded.is_empty() || self.row > 0 {
                    self.gi += 1;
                    if self.gi >= self.groups.len() {
                        self.decoded.clear();
                        return true;
                    }
                }
                self.load_group(b);
                if self.rows_in_group == 0 {
                    self.gi += 1;
                    if self.gi >= self.groups.len() {
                        return true;
                    }
                    self.rows_in_group = 0;
                    self.decoded.clear();
                    self.row = 0;
                    // try the next group on the next loop iteration
                    self.load_group(b);
                    if self.rows_in_group == 0 {
                        continue;
                    }
                }
                continue;
            }
            let mut fields: Vec<(String, Value)> = Vec::with_capacity(self.decoded.len());
            for (ci, vals) in &self.decoded {
                fields.push((self.cols[*ci].name.clone(), vals.get(self.row).cloned().unwrap_or(Value::Null)));
            }
            self.row += 1;
            b.push(&self.name, Value::Obj(fields));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    fn load(name: &str) -> (Vec<Value>, Vec<Value>, Vec<(String, String)>) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
        let mut src = open(&dir.join(format!("{}.parquet", name)), name.to_string(), &Limits::default()).unwrap();
        let mut recs = Vec::new();
        let mut errs = Vec::new();
        loop {
            let mut b = Batch::default();
            let done = src.next(64, 1 << 20, &mut b);
            recs.extend(b.records);
            errs.extend(b.errors);
            if done {
                break;
            }
        }
        let want: Vec<Value> = std::fs::read_to_string(dir.join(format!("{}.jsonl", name))).unwrap().lines().map(|l| json::parse(l).unwrap()).collect();
        (recs, want, errs)
    }

    fn same(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Null, Value::Null) => true,
            (Value::Bool(x), Value::Bool(y)) => x == y,
            (Value::Str(x), Value::Str(y)) => x == y,
            (Value::Num(_), Value::Num(_)) => (a.as_f64().unwrap() - b.as_f64().unwrap()).abs() < 1e-9,
            _ => false,
        }
    }

    #[test]
    fn all_codecs_and_encodings() {
        for name in ["pq_plain", "pq_snappy_dict", "pq_gzip", "pq_zstd_dict"] {
            let (got, want, errs) = load(name);
            assert!(errs.is_empty(), "{}: {:?}", name, errs);
            assert_eq!(got.len(), want.len(), "{}", name);
            for (i, (g, w)) in got.iter().zip(&want).enumerate() {
                for k in ["id", "text", "score", "flag"] {
                    assert!(same(g.get(k).unwrap_or(&Value::Null), w.get(k).unwrap()), "{} row {} field {}: {:?} vs {:?}", name, i, k, g.get(k), w.get(k));
                }
            }
        }
    }

    #[test]
    fn corrupt_and_truncated_files() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
        let raw = std::fs::read(dir.join("pq_snappy_dict.parquet")).unwrap();
        let tmp = std::env::temp_dir().join(format!("mnx_pq_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let cut = tmp.join("cut.parquet");
        std::fs::write(&cut, &raw[..raw.len() / 2]).unwrap();
        assert!(open(&cut, "cut".into(), &Limits::default()).is_err());
        // damage page data inside the first column chunk: the group reports an error, nothing panics
        let mut bad = raw.clone();
        for i in 40..80 {
            bad[i] ^= 0xA5;
        }
        let p = tmp.join("bad.parquet");
        std::fs::write(&p, &bad).unwrap();
        if let Ok(mut src) = open(&p, "bad".into(), &Limits::default()) {
            loop {
                let mut b = Batch::default();
                if src.next(100, 1 << 20, &mut b) {
                    break;
                }
            }
        }
    }

    #[test]
    fn snappy_roundtrip_of_handwritten_block() {
        // "abcabcabc" as literal "abc" + copy(len 6, offset 3)
        let block = [9u8, 2 << 2, b'a', b'b', b'c', ((6 - 1) << 2) | 2, 3, 0];
        assert_eq!(snappy(&block, 100).unwrap(), b"abcabcabc");
        assert!(snappy(&block[..6], 100).is_err());
    }
}
