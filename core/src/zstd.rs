//! Zstandard (RFC 8878) streaming decoder implementing `std::io::Read`. Pure std.
//! Memory is bounded by the frame window (capped by `max_window`) plus one 128 KiB block.
//! Not supported: dictionaries (reported as an error).

use crate::hash::Xxh64;
use std::io::{self, Read};

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("zstd: {}", msg))
}

const BLOCK_MAX: usize = 128 * 1024;

// ---------------------------------------------------------------- backward bit stream

struct Rev<'a> {
    d: &'a [u8],
    pos: i64, // bits still unread
}

impl<'a> Rev<'a> {
    fn new(d: &'a [u8]) -> io::Result<Rev<'a>> {
        let last = *d.last().ok_or_else(|| bad("empty bit stream"))?;
        if last == 0 {
            return Err(bad("bit stream has no end marker"));
        }
        let idx = 7 - last.leading_zeros() as i64;
        Ok(Rev { d, pos: (d.len() as i64 - 1) * 8 + idx })
    }
    fn extract(&self, start: i64, n: u32) -> u64 {
        let byte = (start / 8) as usize;
        let shift = (start % 8) as u32;
        let mut w = 0u64;
        for i in 0..8 {
            if byte + i < self.d.len() {
                w |= (self.d[byte + i] as u64) << (8 * i);
            }
        }
        (w >> shift) & if n >= 64 { u64::MAX } else { (1u64 << n) - 1 }
    }
    fn read(&mut self, n: u32) -> u64 {
        if n == 0 {
            return 0;
        }
        self.pos -= n as i64;
        if self.pos >= 0 {
            self.extract(self.pos, n)
        } else {
            let have = n as i64 + self.pos;
            if have <= 0 {
                0
            } else {
                self.extract(0, have as u32) << (-self.pos)
            }
        }
    }
    fn overflow(&self) -> bool {
        self.pos < 0
    }
    fn peek(&self, n: u32) -> u64 {
        if self.pos >= n as i64 {
            self.extract(self.pos - n as i64, n)
        } else if self.pos > 0 {
            self.extract(0, self.pos as u32) << (n as i64 - self.pos)
        } else {
            0
        }
    }
}

// ---------------------------------------------------------------- FSE

#[derive(Clone)]
struct Fse {
    al: u32,
    sym: Vec<u8>,
    nb: Vec<u8>,
    base: Vec<u16>,
}

fn get_bits(d: &[u8], bp: usize, n: u32) -> u32 {
    let mut w = 0u64;
    let byte = bp / 8;
    for i in 0..8 {
        if byte + i < d.len() {
            w |= (d[byte + i] as u64) << (8 * i);
        }
    }
    ((w >> (bp % 8)) & ((1u64 << n) - 1)) as u32
}

/// Reads a normalized count table. Returns (counts, accuracy log, bytes consumed).
fn read_ncount(d: &[u8], max_sym: usize, max_al: u32) -> io::Result<(Vec<i16>, u32, usize)> {
    if d.is_empty() {
        return Err(bad("empty FSE table description"));
    }
    let al = 5 + get_bits(d, 0, 4);
    if al > max_al {
        return Err(bad("FSE accuracy log too large"));
    }
    let mut bp = 4usize;
    let mut remaining: i32 = (1 << al) + 1;
    let mut threshold: i32 = 1 << al;
    let mut nb = al + 1;
    let mut counts: Vec<i16> = Vec::new();
    let mut prev0 = false;
    while remaining > 1 {
        if prev0 {
            loop {
                if bp / 8 > d.len() + 1 {
                    return Err(bad("truncated FSE table"));
                }
                let r = get_bits(d, bp, 2);
                bp += 2;
                for _ in 0..r {
                    counts.push(0);
                }
                if r != 3 {
                    break;
                }
                if counts.len() > max_sym + 1 {
                    return Err(bad("too many FSE symbols"));
                }
            }
        }
        if counts.len() > max_sym {
            return Err(bad("too many FSE symbols"));
        }
        let max = 2 * threshold - 1 - remaining;
        let low = get_bits(d, bp, nb - 1) as i32;
        let mut count;
        if low < max {
            count = low;
            bp += (nb - 1) as usize;
        } else {
            let v = get_bits(d, bp, nb) as i32;
            count = if v >= threshold { v - max } else { v };
            bp += nb as usize;
        }
        count -= 1;
        remaining -= count.abs();
        counts.push(count as i16);
        prev0 = count == 0;
        if remaining < 1 {
            return Err(bad("corrupt FSE counts"));
        }
        while remaining < threshold {
            nb -= 1;
            threshold >>= 1;
        }
        if bp / 8 > d.len() + 1 {
            return Err(bad("truncated FSE table"));
        }
    }
    if remaining != 1 || counts.len() > max_sym + 1 {
        return Err(bad("corrupt FSE table"));
    }
    let used = bp.div_ceil(8);
    if used > d.len() {
        return Err(bad("truncated FSE table"));
    }
    Ok((counts, al, used))
}

fn build_fse(counts: &[i16], al: u32) -> io::Result<Fse> {
    let size = 1usize << al;
    let mut sym = vec![0u8; size];
    let mut next = vec![0u32; counts.len()];
    let mut high = size as i64 - 1;
    for (s, &c) in counts.iter().enumerate() {
        if c == -1 {
            if high < 0 {
                return Err(bad("FSE table overflow"));
            }
            sym[high as usize] = s as u8;
            high -= 1;
            next[s] = 1;
        } else {
            next[s] = c as u32;
        }
    }
    let step = (size >> 1) + (size >> 3) + 3;
    let mask = size - 1;
    let mut pos = 0usize;
    for (s, &c) in counts.iter().enumerate() {
        for _ in 0..c.max(0) {
            sym[pos] = s as u8;
            pos = (pos + step) & mask;
            while pos as i64 > high {
                pos = (pos + step) & mask;
            }
        }
    }
    if pos != 0 {
        return Err(bad("FSE spread did not close"));
    }
    let mut nb = vec![0u8; size];
    let mut base = vec![0u16; size];
    for u in 0..size {
        let s = sym[u] as usize;
        let n = next[s];
        next[s] += 1;
        let bits = al - (31 - n.leading_zeros());
        nb[u] = bits as u8;
        base[u] = ((n << bits) as usize - size) as u16;
    }
    Ok(Fse { al, sym, nb, base })
}

fn rle_fse(symbol: u8) -> Fse {
    Fse { al: 0, sym: vec![symbol], nb: vec![0], base: vec![0] }
}

const LL_DEFAULT: [i16; 36] = [4, 3, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 2, 1, 1, 1, 1, 1, -1, -1, -1, -1];
const ML_DEFAULT: [i16; 53] = [1, 4, 3, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1, -1, -1, -1];
const OF_DEFAULT: [i16; 29] = [1, 1, 1, 1, 1, 1, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1];

const LL_BASE: [u32; 36] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 18, 20, 22, 24, 28, 32, 40, 48, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536];
const LL_BITS: [u8; 36] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 3, 3, 4, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
const ML_BASE: [u32; 53] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 37, 39, 41, 43, 47, 51, 59, 67, 83, 99, 131, 259, 515, 1027, 2051, 4099, 8195, 16387, 32771, 65539,
];
const ML_BITS: [u8; 53] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 3, 3, 4, 4, 5, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];

// ---------------------------------------------------------------- Huffman

#[derive(Clone)]
struct Huf {
    bits: u32,
    table: Vec<(u8, u8)>, // (symbol, nbits) indexed by the next `bits` bits
}

fn huf_from_weights(mut w: Vec<u8>) -> io::Result<Huf> {
    let mut total: u32 = 0;
    for &x in &w {
        if x > 11 {
            return Err(bad("huffman weight too large"));
        }
        if x > 0 {
            total += 1 << (x - 1);
        }
    }
    if total == 0 {
        return Err(bad("empty huffman table"));
    }
    let maxbits = 32 - total.leading_zeros();
    if maxbits > 11 {
        return Err(bad("huffman table too deep"));
    }
    let rest = (1u32 << maxbits) - total;
    if rest == 0 || !rest.is_power_of_two() {
        return Err(bad("huffman weights do not complete the code"));
    }
    w.push((rest.trailing_zeros() + 1) as u8);
    let size = 1usize << maxbits;
    let mut start = vec![0usize; 13];
    let mut count = vec![0usize; 13];
    for &x in &w {
        count[x as usize] += 1;
    }
    let mut acc = 0usize;
    for wt in 1..=maxbits as usize {
        start[wt] = acc;
        acc += count[wt] << (wt - 1);
    }
    if acc != size {
        return Err(bad("huffman code is not complete"));
    }
    let mut table = vec![(0u8, 0u8); size];
    for (s, &x) in w.iter().enumerate() {
        if x == 0 {
            continue;
        }
        let x = x as usize;
        let n = 1usize << (x - 1);
        let nb = (maxbits as usize + 1 - x) as u8;
        for e in &mut table[start[x]..start[x] + n] {
            *e = (s as u8, nb);
        }
        start[x] += n;
    }
    Ok(Huf { bits: maxbits, table })
}

fn read_huf_tree(d: &[u8]) -> io::Result<(Huf, usize)> {
    let hdr = *d.first().ok_or_else(|| bad("empty huffman header"))? as usize;
    let mut weights = Vec::new();
    let used;
    if hdr >= 128 {
        let n = hdr - 127;
        let bytes = n.div_ceil(2);
        if 1 + bytes > d.len() {
            return Err(bad("truncated huffman weights"));
        }
        for i in 0..n {
            let b = d[1 + i / 2];
            weights.push(if i % 2 == 0 { b >> 4 } else { b & 15 });
        }
        used = 1 + bytes;
    } else {
        if hdr == 0 || 1 + hdr > d.len() {
            return Err(bad("bad compressed huffman header"));
        }
        let comp = &d[1..1 + hdr];
        let (counts, al, nc) = read_ncount(comp, 12, 6)?;
        let t = build_fse(&counts, al)?;
        let mut rb = Rev::new(&comp[nc..])?;
        let mut s1 = rb.read(al) as usize;
        let mut s2 = rb.read(al) as usize;
        loop {
            weights.push(t.sym[s1]);
            s1 = t.base[s1] as usize + rb.read(t.nb[s1] as u32) as usize;
            if rb.overflow() {
                weights.push(t.sym[s2]);
                break;
            }
            weights.push(t.sym[s2]);
            s2 = t.base[s2] as usize + rb.read(t.nb[s2] as u32) as usize;
            if rb.overflow() {
                weights.push(t.sym[s1]);
                break;
            }
            if weights.len() > 255 {
                return Err(bad("too many huffman weights"));
            }
        }
        used = 1 + hdr;
    }
    if weights.len() > 255 {
        return Err(bad("too many huffman weights"));
    }
    Ok((huf_from_weights(weights)?, used))
}

fn huf_stream(h: &Huf, d: &[u8], n: usize, out: &mut Vec<u8>) -> io::Result<()> {
    let mut rb = Rev::new(d)?;
    for _ in 0..n {
        let (s, nb) = h.table[rb.peek(h.bits) as usize];
        rb.pos -= nb as i64;
        if rb.pos < 0 {
            return Err(bad("huffman stream overrun"));
        }
        out.push(s);
    }
    if rb.pos != 0 {
        return Err(bad("huffman stream not fully consumed"));
    }
    Ok(())
}

// ---------------------------------------------------------------- decoder

struct Frame {
    window: usize,
    checksum: bool,
    hash: Xxh64,
    rep: [usize; 3],
    huf: Option<Huf>,
    ll: Option<Fse>,
    of: Option<Fse>,
    ml: Option<Fse>,
}

pub struct Zstd<R: Read> {
    r: R,
    max_window: usize,
    hist: Vec<u8>,
    out_pos: usize,
    frame: Option<Frame>,
    done: bool,
    pub total_out: u64,
}

impl<R: Read> Zstd<R> {
    /// `max_window` caps the history buffer; frames that need more are rejected with a clear error.
    pub fn new(r: R, max_window: usize) -> Self {
        Zstd { r, max_window: max_window.max(1 << 17), hist: Vec::new(), out_pos: 0, frame: None, done: false, total_out: 0 }
    }

    fn read_n(&mut self, n: usize) -> io::Result<Vec<u8>> {
        let mut v = vec![0u8; n];
        self.r.read_exact(&mut v).map_err(|e| if e.kind() == io::ErrorKind::UnexpectedEof { bad("truncated stream") } else { e })?;
        Ok(v)
    }

    fn start_frame(&mut self) -> io::Result<bool> {
        loop {
            let mut m = [0u8; 4];
            let mut got = 0;
            while got < 4 {
                let k = self.r.read(&mut m[got..])?;
                if k == 0 {
                    break;
                }
                got += k;
            }
            if got == 0 {
                return Ok(false);
            }
            if got < 4 {
                return Err(bad("truncated frame header"));
            }
            let magic = u32::from_le_bytes(m);
            if magic & 0xFFFF_FFF0 == 0x184D_2A50 {
                let s = self.read_n(4)?;
                let mut left = u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as u64;
                let mut sink = [0u8; 4096];
                while left > 0 {
                    let k = self.r.read(&mut sink[..(left as usize).min(4096)])?;
                    if k == 0 {
                        return Err(bad("truncated skippable frame"));
                    }
                    left -= k as u64;
                }
                continue;
            }
            if magic != 0xFD2F_B528 {
                return Err(bad("not a zstd stream (bad magic)"));
            }
            break;
        }
        let fd = self.read_n(1)?[0];
        let fcs_flag = fd >> 6;
        let single = fd & 0x20 != 0;
        let checksum = fd & 0x04 != 0;
        let dict_flag = fd & 3;
        if fd & 0x08 != 0 {
            return Err(bad("reserved bit set"));
        }
        let mut window = 0usize;
        if !single {
            let wd = self.read_n(1)?[0];
            let base = 1usize << (10 + (wd >> 3) as usize);
            window = base + (base / 8) * (wd & 7) as usize;
        }
        let dict_len = [0usize, 1, 2, 4][dict_flag as usize];
        let dict = self.read_n(dict_len)?;
        if dict.iter().any(|&b| b != 0) {
            return Err(bad("this file needs a dictionary, which is not supported"));
        }
        let fcs_len = match fcs_flag {
            0 => usize::from(single),
            1 => 2,
            2 => 4,
            _ => 8,
        };
        let f = self.read_n(fcs_len)?;
        let mut content = 0u64;
        for (i, b) in f.iter().enumerate() {
            content |= (*b as u64) << (8 * i);
        }
        if fcs_flag == 1 {
            content += 256;
        }
        if single {
            window = content as usize;
        }
        if window > self.max_window {
            return Err(bad(&format!("frame window {} MiB exceeds the {} MiB memory limit", window >> 20, self.max_window >> 20)));
        }
        self.frame = Some(Frame { window: window.max(BLOCK_MAX), checksum, hash: Xxh64::new(), rep: [1, 4, 8], huf: None, ll: None, of: None, ml: None });
        Ok(true)
    }

    fn trim(&mut self) {
        let keep = self.frame.as_ref().map(|f| f.window).unwrap_or(BLOCK_MAX);
        if self.out_pos == self.hist.len() && self.hist.len() > keep + 2 * BLOCK_MAX {
            let cut = self.hist.len() - keep;
            self.hist.drain(..cut);
            self.out_pos = self.hist.len();
        }
    }

    /// Decodes one block (or a frame boundary). Returns false at the end of the stream.
    fn step(&mut self) -> io::Result<bool> {
        if self.frame.is_none() && !self.start_frame()? {
            return Ok(false);
        }
        self.trim();
        let h = self.read_n(3)?;
        let hv = h[0] as usize | (h[1] as usize) << 8 | (h[2] as usize) << 16;
        let last = hv & 1 == 1;
        let btype = (hv >> 1) & 3;
        let size = hv >> 3;
        let before = self.hist.len();
        match btype {
            0 => {
                if size > BLOCK_MAX {
                    return Err(bad("raw block too large"));
                }
                let d = self.read_n(size)?;
                self.hist.extend_from_slice(&d);
            }
            1 => {
                if size > BLOCK_MAX {
                    return Err(bad("rle block too large"));
                }
                let b = self.read_n(1)?[0];
                self.hist.resize(before + size, b);
            }
            2 => {
                if size > BLOCK_MAX {
                    return Err(bad("compressed block too large"));
                }
                let d = self.read_n(size)?;
                self.decode_compressed(&d)?;
            }
            _ => return Err(bad("reserved block type")),
        }
        let f = self.frame.as_mut().unwrap();
        if f.checksum {
            f.hash.update(&self.hist[before..]);
        }
        if last {
            let f = self.frame.take().unwrap();
            if f.checksum {
                let c = self.read_n(4)?;
                let want = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                if want != (f.hash.finish() & 0xFFFF_FFFF) as u32 {
                    return Err(bad("content checksum mismatch (corrupt data)"));
                }
            }
        }
        Ok(true)
    }

    fn read_literals(&mut self, b: &[u8]) -> io::Result<(Vec<u8>, usize)> {
        if b.is_empty() {
            return Err(bad("empty block"));
        }
        let t = b[0] & 3;
        let sf = (b[0] >> 2) & 3;
        let need = |n: usize| -> io::Result<()> {
            if b.len() < n {
                Err(bad("truncated literals header"))
            } else {
                Ok(())
            }
        };
        if t < 2 {
            let (size, hl) = match sf {
                0 | 2 => ((b[0] >> 3) as usize, 1),
                1 => {
                    need(2)?;
                    ((b[0] as usize | (b[1] as usize) << 8) >> 4, 2)
                }
                _ => {
                    need(3)?;
                    ((b[0] as usize | (b[1] as usize) << 8 | (b[2] as usize) << 16) >> 4, 3)
                }
            };
            if size > BLOCK_MAX {
                return Err(bad("literals too large"));
            }
            if t == 0 {
                need(hl + size)?;
                return Ok((b[hl..hl + size].to_vec(), hl + size));
            }
            need(hl + 1)?;
            return Ok((vec![b[hl]; size], hl + 1));
        }
        let (rs, cs, hl, streams) = match sf {
            0 | 1 => {
                need(3)?;
                let v = b[0] as usize | (b[1] as usize) << 8 | (b[2] as usize) << 16;
                ((v >> 4) & 0x3FF, (v >> 14) & 0x3FF, 3, if sf == 0 { 1 } else { 4 })
            }
            2 => {
                need(4)?;
                let v = b[0] as usize | (b[1] as usize) << 8 | (b[2] as usize) << 16 | (b[3] as usize) << 24;
                ((v >> 4) & 0x3FFF, (v >> 18) & 0x3FFF, 4, 4)
            }
            _ => {
                need(5)?;
                let v = b[0] as usize | (b[1] as usize) << 8 | (b[2] as usize) << 16 | (b[3] as usize) << 24 | (b[4] as usize) << 32;
                ((v >> 4) & 0x3FFFF, (v >> 22) & 0x3FFFF, 5, 4)
            }
        };
        if rs > BLOCK_MAX {
            return Err(bad("literals too large"));
        }
        need(hl + cs)?;
        let mut data = &b[hl..hl + cs];
        let frame = self.frame.as_mut().unwrap();
        if t == 2 {
            let (h, used) = read_huf_tree(data)?;
            frame.huf = Some(h);
            data = &data[used..];
        }
        let huf = frame.huf.as_ref().ok_or_else(|| bad("literals reuse a huffman table that was never sent"))?;
        let mut out = Vec::with_capacity(rs);
        if streams == 1 {
            huf_stream(huf, data, rs, &mut out)?;
        } else {
            if data.len() < 6 {
                return Err(bad("truncated huffman jump table"));
            }
            let s1 = data[0] as usize | (data[1] as usize) << 8;
            let s2 = data[2] as usize | (data[3] as usize) << 8;
            let s3 = data[4] as usize | (data[5] as usize) << 8;
            let rest = &data[6..];
            if s1 + s2 + s3 > rest.len() {
                return Err(bad("huffman stream sizes exceed the section"));
            }
            let seg = rs.div_ceil(4);
            let parts = [&rest[..s1], &rest[s1..s1 + s2], &rest[s1 + s2..s1 + s2 + s3], &rest[s1 + s2 + s3..]];
            let counts = [seg, seg, seg, rs.saturating_sub(3 * seg)];
            for (p, c) in parts.iter().zip(counts) {
                huf_stream(huf, p, c, &mut out)?;
            }
        }
        Ok((out, hl + cs))
    }

    fn decode_compressed(&mut self, blk: &[u8]) -> io::Result<()> {
        let block_start = self.hist.len();
        let (lits, used) = self.read_literals(blk)?;
        let seq = &blk[used..];
        if seq.is_empty() {
            return Err(bad("missing sequences section"));
        }
        let b0 = seq[0] as usize;
        let (nseq, mut p) = if b0 == 0 {
            self.hist.extend_from_slice(&lits);
            return Ok(());
        } else if b0 < 128 {
            (b0, 1)
        } else if b0 < 255 {
            if seq.len() < 2 {
                return Err(bad("truncated sequences header"));
            }
            (((b0 - 128) << 8) + seq[1] as usize, 2)
        } else {
            if seq.len() < 3 {
                return Err(bad("truncated sequences header"));
            }
            (seq[1] as usize + ((seq[2] as usize) << 8) + 0x7F00, 3)
        };
        if p >= seq.len() {
            return Err(bad("truncated sequences header"));
        }
        let modes = seq[p];
        p += 1;
        let frame = self.frame.as_mut().unwrap();
        let mut tables: Vec<Fse> = Vec::with_capacity(3);
        for (i, (max_sym, max_al, default, prev)) in [(35usize, 9u32, &LL_DEFAULT[..], 0usize), (31, 8, &OF_DEFAULT[..], 1), (52, 9, &ML_DEFAULT[..], 2)].into_iter().enumerate() {
            let mode = (modes >> (6 - 2 * i)) & 3;
            let default_al = if i == 1 { 5 } else { 6 };
            let t = match mode {
                0 => build_fse(default, default_al)?,
                1 => {
                    let s = *seq.get(p).ok_or_else(|| bad("truncated RLE table"))?;
                    p += 1;
                    if s as usize > max_sym {
                        return Err(bad("RLE symbol out of range"));
                    }
                    rle_fse(s)
                }
                2 => {
                    let (c, al, u) = read_ncount(&seq[p..], max_sym, max_al)?;
                    p += u;
                    build_fse(&c, al)?
                }
                _ => match prev {
                    0 => frame.ll.clone(),
                    1 => frame.of.clone(),
                    _ => frame.ml.clone(),
                }
                .ok_or_else(|| bad("repeat mode without a previous table"))?,
            };
            tables.push(t);
        }
        let (tml, tof, tll) = (tables.pop().unwrap(), tables.pop().unwrap(), tables.pop().unwrap());
        let mut rb = Rev::new(&seq[p..])?;
        let mut sll = rb.read(tll.al) as usize;
        let mut sof = rb.read(tof.al) as usize;
        let mut sml = rb.read(tml.al) as usize;
        let mut rep = frame.rep;
        let mut lp = 0usize;
        for i in 0..nseq {
            let ofc = tof.sym[sof] as u32;
            let mlc = tml.sym[sml] as usize;
            let llc = tll.sym[sll] as usize;
            if mlc >= ML_BASE.len() || llc >= LL_BASE.len() || ofc > 31 {
                return Err(bad("sequence code out of range"));
            }
            let ov = (1u64 << ofc) + rb.read(ofc);
            let ml = ML_BASE[mlc] as usize + rb.read(ML_BITS[mlc] as u32) as usize;
            let ll = LL_BASE[llc] as usize + rb.read(LL_BITS[llc] as u32) as usize;
            if i + 1 != nseq {
                sll = tll.base[sll] as usize + rb.read(tll.nb[sll] as u32) as usize;
                sml = tml.base[sml] as usize + rb.read(tml.nb[sml] as u32) as usize;
                sof = tof.base[sof] as usize + rb.read(tof.nb[sof] as u32) as usize;
            }
            if rb.overflow() {
                return Err(bad("sequence stream overrun"));
            }
            if lp + ll > lits.len() {
                return Err(bad("sequence reads past the literals"));
            }
            self.hist.extend_from_slice(&lits[lp..lp + ll]);
            lp += ll;
            let offset;
            if ov > 3 {
                offset = (ov - 3) as usize;
                rep = [offset, rep[0], rep[1]];
            } else {
                let idx = (ov - 1) as usize + usize::from(ll == 0);
                if idx == 0 {
                    offset = rep[0];
                } else {
                    offset = if idx == 3 { rep[0].saturating_sub(1) } else { rep[idx] };
                    if idx == 1 {
                        rep[1] = rep[0];
                    } else {
                        rep[2] = rep[1];
                        rep[1] = rep[0];
                    }
                    rep[0] = offset;
                }
            }
            if offset == 0 || offset > self.hist.len() {
                return Err(bad("match offset reaches before the available history"));
            }
            if self.hist.len() - block_start + ml > BLOCK_MAX + 8 {
                return Err(bad("block expands beyond the maximum block size"));
            }
            let start = self.hist.len() - offset;
            if offset >= ml {
                self.hist.extend_from_within(start..start + ml);
            } else {
                for k in 0..ml {
                    let b = self.hist[start + k];
                    self.hist.push(b);
                }
            }
        }
        if rb.pos != 0 {
            return Err(bad("sequence stream not fully consumed (corrupt block)"));
        }
        self.hist.extend_from_slice(&lits[lp..]);
        if self.hist.len() - block_start > BLOCK_MAX + 8 {
            return Err(bad("block expands beyond the maximum block size"));
        }
        let f = self.frame.as_mut().unwrap();
        f.rep = rep;
        f.ll = Some(tll);
        f.of = Some(tof);
        f.ml = Some(tml);
        Ok(())
    }
}

impl<R: Read> Read for Zstd<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            if self.out_pos < self.hist.len() {
                let n = out.len().min(self.hist.len() - self.out_pos);
                out[..n].copy_from_slice(&self.hist[self.out_pos..self.out_pos + n]);
                self.out_pos += n;
                self.total_out += n as u64;
                return Ok(n);
            }
            if self.done {
                return Ok(0);
            }
            if !self.step()? {
                self.done = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::Sha256;

    fn sha(d: &[u8]) -> String {
        let mut s = Sha256::new();
        s.update(d);
        s.finish_hex()
    }

    #[test]
    fn rle_and_raw_blocks() {
        // frame: magic, single segment, content size 5, one RLE block ("a" x5) then last raw block "bc"
        let mut f = vec![0x28, 0xB5, 0x2F, 0xFD, 0x20, 0x07];
        f.extend_from_slice(&[(5u32 << 3 | 1 << 1) as u8, 0, 0, b'a']);
        f.extend_from_slice(&[(2u32 << 3 | 1) as u8, 0, 0, b'b', b'c']);
        let mut out = Vec::new();
        Zstd::new(&f[..], 1 << 26).read_to_end(&mut out).unwrap();
        assert_eq!(out, b"aaaaabc");
    }

    #[test]
    fn real_vectors() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
        let mut checked = 0;
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if p.extension().and_then(|x| x.to_str()) != Some("zst") {
                continue;
            }
            let want = std::fs::read_to_string(p.with_extension("sha256")).unwrap();
            let mut out = Vec::new();
            let f = std::fs::File::open(&p).unwrap();
            Zstd::new(std::io::BufReader::new(f), 1 << 27).read_to_end(&mut out).unwrap_or_else(|e| panic!("{:?}: {}", p, e));
            assert_eq!(sha(&out), want.trim(), "{:?}", p);
            checked += 1;
        }
        assert!(checked >= 5, "expected the bundled zstd vectors");
    }

    #[test]
    fn corruption_and_truncation_are_errors() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
        let data = std::fs::read(dir.join("text_l3.zst")).unwrap();
        let mut out = Vec::new();
        assert!(Zstd::new(&data[..data.len() / 2], 1 << 26).read_to_end(&mut out).is_err());
        let mut bad = data.clone();
        let mid = bad.len() / 2;
        bad[mid] ^= 0x55;
        let mut out = Vec::new();
        assert!(Zstd::new(&bad[..], 1 << 26).read_to_end(&mut out).is_err());
        assert!(Zstd::new(&b"not zstd at all"[..], 1 << 26).read_to_end(&mut Vec::new()).is_err());
    }
}
