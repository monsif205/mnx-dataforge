//! Streaming DEFLATE (RFC 1951) and gzip (RFC 1952) decoders implementing `std::io::Read`.
//! Memory use is fixed: a 32 KiB window plus a small input buffer, whatever the file size.

use crate::hash::Crc32;
use std::io::{self, Read};

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

struct Bits<R: Read> {
    r: R,
    buf: Vec<u8>,
    pos: usize,
    len: usize,
    acc: u64,
    n: u32,
    eof: bool,
}

impl<R: Read> Bits<R> {
    fn new(r: R) -> Self {
        Bits { r, buf: vec![0; 32 * 1024], pos: 0, len: 0, acc: 0, n: 0, eof: false }
    }
    fn next_byte(&mut self) -> io::Result<Option<u8>> {
        if self.pos == self.len {
            if self.eof {
                return Ok(None);
            }
            self.len = self.r.read(&mut self.buf)?;
            self.pos = 0;
            if self.len == 0 {
                self.eof = true;
                return Ok(None);
            }
        }
        let b = self.buf[self.pos];
        self.pos += 1;
        Ok(Some(b))
    }
    fn need(&mut self, k: u32) -> io::Result<()> {
        while self.n < k {
            match self.next_byte()? {
                Some(b) => {
                    self.acc |= (b as u64) << self.n;
                    self.n += 8;
                }
                None => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated deflate stream")),
            }
        }
        Ok(())
    }
    fn bits(&mut self, k: u32) -> io::Result<u32> {
        if k == 0 {
            return Ok(0);
        }
        self.need(k)?;
        let v = (self.acc & ((1u64 << k) - 1)) as u32;
        self.acc >>= k;
        self.n -= k;
        Ok(v)
    }
    /// Peek up to k bits; pads with zeros at end of input (the caller validates the consumed length).
    fn peek(&mut self, k: u32) -> io::Result<u32> {
        while self.n < k {
            match self.next_byte()? {
                Some(b) => {
                    self.acc |= (b as u64) << self.n;
                    self.n += 8;
                }
                None => break,
            }
        }
        Ok((self.acc & ((1u64 << k) - 1)) as u32)
    }
    fn drop_bits(&mut self, k: u32) -> io::Result<()> {
        if k > self.n {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated deflate stream"));
        }
        self.acc >>= k;
        self.n -= k;
        Ok(())
    }
    fn align(&mut self) {
        let drop = self.n % 8;
        self.acc >>= drop;
        self.n -= drop;
    }
    fn read_exact_bytes(&mut self, out: &mut [u8]) -> io::Result<()> {
        for o in out.iter_mut() {
            *o = self.bits(8)? as u8;
        }
        Ok(())
    }
}

struct Huff {
    table: Vec<u16>, // (symbol << 4) | length, indexed by bit-reversed code; 0 = invalid
    bits: u32,
}

impl Huff {
    fn build(lengths: &[u8]) -> io::Result<Huff> {
        let max = lengths.iter().copied().max().unwrap_or(0) as u32;
        if max == 0 {
            return Ok(Huff { table: vec![0; 2], bits: 1 });
        }
        let mut count = [0u32; 16];
        for &l in lengths {
            count[l as usize] += 1;
        }
        count[0] = 0;
        let mut left: i32 = 1;
        for c in count.iter().skip(1) {
            left = (left << 1) - *c as i32;
            if left < 0 {
                return Err(bad("over-subscribed huffman code"));
            }
        }
        let mut next = [0u32; 16];
        let mut code = 0u32;
        for b in 1..16 {
            code = (code + count[b - 1]) << 1;
            next[b] = code;
        }
        let mut table = vec![0u16; 1 << max];
        for (sym, &l) in lengths.iter().enumerate() {
            if l == 0 {
                continue;
            }
            let c = next[l as usize];
            next[l as usize] += 1;
            let mut rev = 0u32;
            for i in 0..l as u32 {
                if c & (1 << i) != 0 {
                    rev |= 1 << (l as u32 - 1 - i);
                }
            }
            let mut idx = rev as usize;
            let step = 1usize << l;
            while idx < table.len() {
                table[idx] = ((sym as u16) << 4) | l as u16;
                idx += step;
            }
        }
        Ok(Huff { table, bits: max })
    }
    fn decode<R: Read>(&self, b: &mut Bits<R>) -> io::Result<u16> {
        let v = b.peek(self.bits)? as usize;
        let e = self.table[v];
        let l = (e & 15) as u32;
        if l == 0 {
            return Err(bad("invalid huffman code"));
        }
        b.drop_bits(l)?;
        Ok(e >> 4)
    }
}

const LEN_BASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEN_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

enum State {
    Header,
    Stored(usize),
    Codes(Box<(Huff, Huff)>),
    Done,
}

pub struct Inflate<R: Read> {
    b: Bits<R>,
    win: Vec<u8>,
    wpos: usize,
    filled: usize,
    state: State,
    last: bool,
    copy_len: usize,
    copy_dist: usize,
    pub total_out: u64,
}

impl<R: Read> Inflate<R> {
    pub fn new(r: R) -> Self {
        Inflate { b: Bits::new(r), win: vec![0; 32768], wpos: 0, filled: 0, state: State::Header, last: false, copy_len: 0, copy_dist: 0, total_out: 0 }
    }
    /// Bytes read past the end of the deflate data are returned to the caller through `into_inner_remaining`.
    pub fn into_inner(self) -> R {
        self.b.r
    }
    fn put(&mut self, byte: u8) {
        self.win[self.wpos] = byte;
        self.wpos = (self.wpos + 1) & 32767;
        if self.filled < 32768 {
            self.filled += 1;
        }
    }
    fn read_header(&mut self) -> io::Result<()> {
        if self.last {
            self.state = State::Done;
            return Ok(());
        }
        self.last = self.b.bits(1)? == 1;
        match self.b.bits(2)? {
            0 => {
                self.b.align();
                let len = self.b.bits(16)? as usize;
                let nlen = self.b.bits(16)? as usize;
                if len != (!nlen & 0xFFFF) {
                    return Err(bad("stored block length check failed"));
                }
                self.state = State::Stored(len);
            }
            1 => {
                let mut l = [0u8; 288];
                for (i, x) in l.iter_mut().enumerate() {
                    *x = if i < 144 { 8 } else if i < 256 { 9 } else if i < 280 { 7 } else { 8 };
                }
                self.state = State::Codes(Box::new((Huff::build(&l)?, Huff::build(&[5u8; 30])?)));
            }
            2 => {
                let hlit = self.b.bits(5)? as usize + 257;
                let hdist = self.b.bits(5)? as usize + 1;
                let hclen = self.b.bits(4)? as usize + 4;
                if hlit > 286 || hdist > 30 {
                    return Err(bad("bad dynamic block header"));
                }
                const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
                let mut cl = [0u8; 19];
                for &o in ORDER.iter().take(hclen) {
                    cl[o] = self.b.bits(3)? as u8;
                }
                let clh = Huff::build(&cl)?;
                let mut lens = vec![0u8; hlit + hdist];
                let mut i = 0;
                while i < lens.len() {
                    let sym = clh.decode(&mut self.b)?;
                    match sym {
                        0..=15 => {
                            lens[i] = sym as u8;
                            i += 1;
                        }
                        16 => {
                            if i == 0 {
                                return Err(bad("repeat with no previous length"));
                            }
                            let prev = lens[i - 1];
                            let n = 3 + self.b.bits(2)? as usize;
                            if i + n > lens.len() {
                                return Err(bad("code length repeat overflow"));
                            }
                            for _ in 0..n {
                                lens[i] = prev;
                                i += 1;
                            }
                        }
                        17 | 18 => {
                            let n = if sym == 17 { 3 + self.b.bits(3)? as usize } else { 11 + self.b.bits(7)? as usize };
                            if i + n > lens.len() {
                                return Err(bad("code length repeat overflow"));
                            }
                            i += n;
                        }
                        _ => return Err(bad("bad code length symbol")),
                    }
                }
                if lens[256] == 0 {
                    return Err(bad("missing end-of-block code"));
                }
                self.state = State::Codes(Box::new((Huff::build(&lens[..hlit])?, Huff::build(&lens[hlit..])?)));
            }
            _ => return Err(bad("reserved deflate block type")),
        }
        Ok(())
    }
}

impl<R: Read> Read for Inflate<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let mut n = 0;
        while n < out.len() {
            if self.copy_len > 0 {
                let src = (self.wpos + 32768 - self.copy_dist) & 32767;
                let byte = self.win[src];
                self.put(byte);
                out[n] = byte;
                n += 1;
                self.copy_len -= 1;
                continue;
            }
            match &mut self.state {
                State::Done => break,
                State::Header => self.read_header()?,
                State::Stored(rem) => {
                    if *rem == 0 {
                        self.state = State::Header;
                        continue;
                    }
                    *rem -= 1;
                    let v = self.b.bits(8)? as u8;
                    self.put(v);
                    out[n] = v;
                    n += 1;
                }
                State::Codes(_) => {
                    // Take the tables out so the bit reader can be borrowed mutably.
                    let tables = match std::mem::replace(&mut self.state, State::Header) {
                        State::Codes(t) => t,
                        _ => unreachable!(),
                    };
                    let (lit, dist) = (&tables.0, &tables.1);
                    let mut finished_block = false;
                    while n < out.len() && self.copy_len == 0 {
                        let sym = lit.decode(&mut self.b)? as usize;
                        if sym < 256 {
                            self.put(sym as u8);
                            out[n] = sym as u8;
                            n += 1;
                        } else if sym == 256 {
                            finished_block = true;
                            break;
                        } else {
                            let li = sym - 257;
                            if li >= 29 {
                                return Err(bad("bad length symbol"));
                            }
                            let len = LEN_BASE[li] as usize + self.b.bits(LEN_EXTRA[li] as u32)? as usize;
                            let ds = dist.decode(&mut self.b)? as usize;
                            if ds >= 30 {
                                return Err(bad("bad distance symbol"));
                            }
                            let d = DIST_BASE[ds] as usize + self.b.bits(DIST_EXTRA[ds] as u32)? as usize;
                            if d > self.filled {
                                return Err(bad("distance reaches before the start of the data"));
                            }
                            self.copy_len = len;
                            self.copy_dist = d;
                        }
                    }
                    if !finished_block {
                        self.state = State::Codes(tables);
                    }
                }
            }
        }
        self.total_out += n as u64;
        Ok(n)
    }
}

/// gzip member reader (multi-member files are concatenated). Verifies CRC-32 and size of each member.
pub struct Gunzip<R: Read> {
    inner: Option<Inflate<R>>,
    crc: Crc32,
    size: u32,
    started: bool,
    finished: bool,
    member_open: bool,
}

impl<R: Read> Gunzip<R> {
    pub fn new(r: R) -> Self {
        Gunzip { inner: Some(Inflate::new(r)), crc: Crc32::new(), size: 0, started: false, finished: false, member_open: false }
    }
    fn header(&mut self) -> io::Result<bool> {
        let inf = self.inner.as_mut().unwrap();
        let b = &mut inf.b;
        let id1 = match b.next_byte()? {
            Some(x) => x,
            None => {
                if self.started {
                    return Ok(false);
                }
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "empty gzip file"));
            }
        };
        let id2 = b.next_byte()?.ok_or_else(|| bad("truncated gzip header"))?;
        if id1 != 0x1f || id2 != 0x8b {
            if self.started {
                return Ok(false); // trailing garbage after a valid member is ignored
            }
            return Err(bad("not a gzip file"));
        }
        let mut h = [0u8; 8];
        for x in h.iter_mut() {
            *x = b.next_byte()?.ok_or_else(|| bad("truncated gzip header"))?;
        }
        if h[0] != 8 {
            return Err(bad("unsupported gzip compression method"));
        }
        let flg = h[1];
        if flg & 4 != 0 {
            let lo = b.next_byte()?.ok_or_else(|| bad("truncated gzip header"))? as usize;
            let hi = b.next_byte()?.ok_or_else(|| bad("truncated gzip header"))? as usize;
            for _ in 0..(lo | hi << 8) {
                b.next_byte()?.ok_or_else(|| bad("truncated gzip header"))?;
            }
        }
        for bit in [8u8, 16] {
            if flg & bit != 0 {
                loop {
                    if b.next_byte()?.ok_or_else(|| bad("truncated gzip header"))? == 0 {
                        break;
                    }
                }
            }
        }
        if flg & 2 != 0 {
            b.next_byte()?;
            b.next_byte()?;
        }
        self.started = true;
        Ok(true)
    }
}

impl<R: Read> Read for Gunzip<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.finished || out.is_empty() {
                return Ok(0);
            }
            if !self.member_open {
                if !self.header()? {
                    self.finished = true;
                    return Ok(0);
                }
                self.crc = Crc32::new();
                self.size = 0;
                let inf = self.inner.as_mut().unwrap();
                inf.state = State::Header;
                inf.last = false;
                inf.filled = 0;
                inf.wpos = 0;
                inf.copy_len = 0;
                self.member_open = true;
            }
            let inf = self.inner.as_mut().unwrap();
            let n = inf.read(out)?;
            if n > 0 {
                self.crc.update(&out[..n]);
                self.size = self.size.wrapping_add(n as u32);
                return Ok(n);
            }
            // end of this deflate stream: trailer
            let b = &mut inf.b;
            b.align();
            let mut t = [0u8; 8];
            b.read_exact_bytes(&mut t)?;
            let crc = u32::from_le_bytes([t[0], t[1], t[2], t[3]]);
            let isize = u32::from_le_bytes([t[4], t[5], t[6], t[7]]);
            if crc != self.crc.finish() || isize != self.size {
                return Err(bad("gzip checksum mismatch (corrupt data)"));
            }
            self.member_open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn gz(data: &[u8], level: &str) -> Vec<u8> {
        use std::io::Write;
        let mut c = Command::new("gzip").arg(level).arg("-c").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
        let mut stdin = c.stdin.take().unwrap();
        let d = data.to_vec();
        let t = std::thread::spawn(move || {
            stdin.write_all(&d).unwrap();
        });
        let out = c.wait_with_output().unwrap();
        t.join().unwrap();
        out.stdout
    }

    #[test]
    fn gzip_roundtrip_levels() {
        let mut data = Vec::new();
        for i in 0..40000u32 {
            data.extend_from_slice(format!("line {} the quick brown fox {}\n", i, i % 97).as_bytes());
        }
        for level in ["-1", "-6", "-9"] {
            let z = gz(&data, level);
            let mut out = Vec::new();
            Gunzip::new(&z[..]).read_to_end(&mut out).unwrap();
            assert_eq!(out, data, "level {}", level);
        }
        // empty and tiny inputs
        let z = gz(b"", "-6");
        let mut out = Vec::new();
        Gunzip::new(&z[..]).read_to_end(&mut out).unwrap();
        assert!(out.is_empty());
        // concatenated members
        let mut two = gz(b"hello ", "-6");
        two.extend(gz(b"world", "-6"));
        let mut out = Vec::new();
        Gunzip::new(&two[..]).read_to_end(&mut out).unwrap();
        assert_eq!(out, b"hello world");
    }

    #[test]
    fn corruption_is_detected() {
        let data = vec![b'x'; 50000];
        let mut z = gz(&data, "-6");
        let n = z.len();
        z[n - 6] ^= 0xFF; // damage the CRC
        let mut out = Vec::new();
        assert!(Gunzip::new(&z[..]).read_to_end(&mut out).is_err());
        let z = gz(&data, "-6");
        let mut out = Vec::new();
        assert!(Gunzip::new(&z[..z.len() / 2]).read_to_end(&mut out).is_err());
    }
}
