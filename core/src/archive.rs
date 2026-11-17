//! Container readers: ZIP (incl. ZIP64, stored/deflate/zstd entries) and TAR (ustar, GNU long names, pax),
//! plus format sniffing. Everything streams with fixed memory; damaged entries are reported, not fatal.

use crate::hash::Crc32;
use crate::inflate::Inflate;
use crate::zstd::Zstd;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

pub fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Zip,
    Tar,
    Gzip,
    Zstd,
    Parquet,
    Plain,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Zip => "zip",
            Kind::Tar => "tar",
            Kind::Gzip => "gzip",
            Kind::Zstd => "zstd",
            Kind::Parquet => "parquet",
            Kind::Plain => "plain",
        }
    }
}

pub fn sniff(head: &[u8]) -> Kind {
    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        Kind::Zip
    } else if head.starts_with(&[0x1f, 0x8b]) {
        Kind::Gzip
    } else if head.len() >= 4 && (head[..4] == [0x28, 0xB5, 0x2F, 0xFD] || (head[0] & 0xF0 == 0x50 && head[1..4] == [0x2A, 0x4D, 0x18])) {
        Kind::Zstd
    } else if head.starts_with(b"PAR1") {
        Kind::Parquet
    } else if head.len() >= 262 && &head[257..262] == b"ustar" {
        Kind::Tar
    } else {
        Kind::Plain
    }
}

/// Lowercase extension without a trailing compression suffix: "a.jsonl.gz" -> "jsonl".
pub fn leaf_ext(name: &str) -> String {
    let mut n = name.to_lowercase();
    for suf in [".gz", ".zst", ".zstd"] {
        if let Some(s) = n.strip_suffix(suf) {
            n = s.to_string();
            break;
        }
    }
    n.rsplit('.').next().filter(|e| e.len() < 12 && n.contains('.')).unwrap_or("").to_string()
}

/// Counts every byte read so progress can be reported against the real file size.
pub struct Counting<R: Read> {
    pub inner: R,
    pub count: Arc<std::sync::atomic::AtomicU64>,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(b)?;
        self.count.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
        Ok(n)
    }
}

/// Fails with an error once more than `limit` bytes have been read (decompression-bomb guard). 0 = unlimited.
pub struct Limited<R: Read> {
    pub inner: R,
    pub left: u64,
    pub unlimited: bool,
    pub what: String,
}

impl<R: Read> Read for Limited<R> {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(b)?;
        if !self.unlimited {
            if n as u64 > self.left {
                return Err(bad(format!("{} is larger than the configured limit", self.what)));
            }
            self.left -= n as u64;
        }
        Ok(n)
    }
}

/// Verifies CRC-32 and length when the wrapped entry reaches its end.
struct CrcCheck<R: Read> {
    inner: R,
    crc: Crc32,
    want_crc: u32,
    want_len: u64,
    got: u64,
    checked: bool,
}

impl<R: Read> Read for CrcCheck<R> {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(b)?;
        if n > 0 {
            self.crc.update(&b[..n]);
            self.got += n as u64;
        } else if !self.checked && !b.is_empty() {
            self.checked = true;
            if self.got != self.want_len || self.crc.finish() != self.want_crc {
                return Err(bad("entry is corrupt (CRC-32 or size mismatch)"));
            }
        }
        Ok(n)
    }
}

// ---------------------------------------------------------------- ZIP

#[derive(Debug, Clone)]
pub struct ZipEntry {
    pub name: String,
    pub size: u64,
    pub csize: u64,
    pub method: u16,
    pub crc: u32,
    pub offset: u64,
    pub encrypted: bool,
}

fn u16le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
fn u64le(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3], b[i + 4], b[i + 5], b[i + 6], b[i + 7]])
}

pub fn zip_entries<F: Read + Seek>(f: &mut F, max_entries: usize) -> io::Result<Vec<ZipEntry>> {
    let len = f.seek(SeekFrom::End(0))?;
    let tail_len = len.min(70_000);
    f.seek(SeekFrom::Start(len - tail_len))?;
    let mut tail = vec![0u8; tail_len as usize];
    f.read_exact(&mut tail)?;
    let eocd = (0..tail.len().saturating_sub(21)).rev().find(|&i| &tail[i..i + 4] == b"PK\x05\x06").ok_or_else(|| bad("not a valid ZIP (no end-of-central-directory record); the file may be truncated"))?;
    let mut total = u16le(&tail, eocd + 10) as u64;
    let mut cd_size = u32le(&tail, eocd + 12) as u64;
    let mut cd_off = u32le(&tail, eocd + 16) as u64;
    if (total == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_off == 0xFFFF_FFFF) && eocd >= 20 && &tail[eocd - 20..eocd - 16] == b"PK\x06\x07" {
        let loc = u64le(&tail, eocd - 20 + 8);
        f.seek(SeekFrom::Start(loc))?;
        let mut z = [0u8; 56];
        f.read_exact(&mut z)?;
        if &z[..4] != b"PK\x06\x06" {
            return Err(bad("bad ZIP64 end record"));
        }
        total = u64le(&z, 32);
        cd_size = u64le(&z, 40);
        cd_off = u64le(&z, 48);
    }
    if cd_off.saturating_add(cd_size) > len {
        return Err(bad("central directory points outside the file (truncated or corrupt ZIP)"));
    }
    if cd_size > 256 * 1024 * 1024 {
        return Err(bad("central directory is unreasonably large"));
    }
    f.seek(SeekFrom::Start(cd_off))?;
    let mut cd = vec![0u8; cd_size as usize];
    f.read_exact(&mut cd)?;
    let mut out = Vec::new();
    let mut p = 0usize;
    while p + 46 <= cd.len() && (out.len() as u64) < total.max(1) * 2 && out.len() < max_entries {
        if &cd[p..p + 4] != b"PK\x01\x02" {
            break;
        }
        let flags = u16le(&cd, p + 8);
        let method = u16le(&cd, p + 10);
        let crc = u32le(&cd, p + 16);
        let mut csize = u32le(&cd, p + 20) as u64;
        let mut size = u32le(&cd, p + 24) as u64;
        let nlen = u16le(&cd, p + 28) as usize;
        let elen = u16le(&cd, p + 30) as usize;
        let clen = u16le(&cd, p + 32) as usize;
        let mut offset = u32le(&cd, p + 42) as u64;
        if p + 46 + nlen + elen + clen > cd.len() {
            break;
        }
        let name_bytes = &cd[p + 46..p + 46 + nlen];
        let name = if flags & 0x800 != 0 { String::from_utf8_lossy(name_bytes).to_string() } else { String::from_utf8(name_bytes.to_vec()).unwrap_or_else(|_| name_bytes.iter().map(|&b| b as char).collect()) };
        let extra = &cd[p + 46 + nlen..p + 46 + nlen + elen];
        let mut e = 0usize;
        while e + 4 <= extra.len() {
            let id = u16le(extra, e);
            let sz = u16le(extra, e + 2) as usize;
            if id == 1 && e + 4 + sz <= extra.len() {
                let mut q = e + 4;
                if size == 0xFFFF_FFFF && q + 8 <= e + 4 + sz {
                    size = u64le(extra, q);
                    q += 8;
                }
                if csize == 0xFFFF_FFFF && q + 8 <= e + 4 + sz {
                    csize = u64le(extra, q);
                    q += 8;
                }
                if offset == 0xFFFF_FFFF && q + 8 <= e + 4 + sz {
                    offset = u64le(extra, q);
                }
            }
            e += 4 + sz;
        }
        out.push(ZipEntry { name, size, csize, method, crc, offset, encrypted: flags & 1 != 0 });
        p += 46 + nlen + elen + clen;
    }
    Ok(out)
}

/// Opens one ZIP entry as a verified, decompressed stream. Memory-light: only the decoder window is held.
pub fn zip_open(path: &std::path::Path, e: &ZipEntry, max_window: usize, count: Arc<std::sync::atomic::AtomicU64>) -> io::Result<Box<dyn Read + Send>> {
    if e.encrypted {
        return Err(bad("entry is encrypted (not supported)"));
    }
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(e.offset))?;
    let mut h = [0u8; 30];
    f.read_exact(&mut h).map_err(|_| bad("truncated local header"))?;
    if &h[..4] != b"PK\x03\x04" {
        return Err(bad("bad local header (corrupt ZIP)"));
    }
    let skip = u16le(&h, 26) as u64 + u16le(&h, 28) as u64;
    f.seek(SeekFrom::Current(skip as i64))?;
    let raw = Counting { inner: BufReader::with_capacity(64 * 1024, f).take(e.csize), count };
    let dec: Box<dyn Read + Send> = match e.method {
        0 => Box::new(raw),
        8 => Box::new(Inflate::new(raw)),
        93 => Box::new(Zstd::new(raw, max_window)),
        m => return Err(bad(format!("unsupported ZIP compression method {}", m))),
    };
    Ok(Box::new(CrcCheck { inner: dec, crc: Crc32::new(), want_crc: e.crc, want_len: e.size, got: 0, checked: false }))
}

// ---------------------------------------------------------------- TAR

#[derive(Debug, Clone)]
pub struct TarEntry {
    pub name: String,
    pub size: u64,
    pub is_file: bool,
}

pub struct TarState {
    r: Box<dyn Read + Send>,
    remaining: u64,
    pad: u64,
    pub ended: bool,
}

pub type SharedTar = Arc<Mutex<TarState>>;

pub fn tar_new(r: Box<dyn Read + Send>) -> SharedTar {
    Arc::new(Mutex::new(TarState { r, remaining: 0, pad: 0, ended: false }))
}

fn discard(r: &mut dyn Read, mut n: u64) -> io::Result<()> {
    let mut sink = [0u8; 8192];
    while n > 0 {
        let k = r.read(&mut sink[..(n as usize).min(8192)])?;
        if k == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "tar archive is truncated"));
        }
        n -= k as u64;
    }
    Ok(())
}

fn octal(b: &[u8]) -> Option<u64> {
    if b.first().map(|x| x & 0x80 != 0).unwrap_or(false) {
        let mut v = (b[0] & 0x7f) as u64;
        for x in &b[1..] {
            v = v.checked_mul(256)?.checked_add(*x as u64)?;
        }
        return Some(v);
    }
    let s: String = b.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect();
    let t = s.trim();
    if t.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(t, 8).ok()
}

fn cstr(b: &[u8]) -> String {
    String::from_utf8_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]).to_string()
}

fn read_block(r: &mut dyn Read, buf: &mut [u8; 512]) -> io::Result<bool> {
    let mut got = 0;
    while got < 512 {
        let k = r.read(&mut buf[got..])?;
        if k == 0 {
            if got == 0 {
                return Ok(false);
            }
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "tar archive is truncated"));
        }
        got += k;
    }
    Ok(true)
}

fn read_exact_vec(r: &mut dyn Read, n: u64, cap: u64) -> io::Result<Vec<u8>> {
    if n > cap {
        return Err(bad("tar metadata record is unreasonably large"));
    }
    let mut v = vec![0u8; n as usize];
    r.read_exact(&mut v)?;
    Ok(v)
}

/// Advances to the next entry. Returns None at the end of the archive.
pub fn tar_next(st: &SharedTar) -> io::Result<Option<TarEntry>> {
    let mut s = st.lock().unwrap();
    if s.ended {
        return Ok(None);
    }
    let skip = s.remaining + s.pad;
    s.remaining = 0;
    s.pad = 0;
    discard(&mut *s.r, skip)?;
    let mut long_name: Option<String> = None;
    let mut pax_path: Option<String> = None;
    let mut pax_size: Option<u64> = None;
    loop {
        let mut h = [0u8; 512];
        if !read_block(&mut *s.r, &mut h)? {
            s.ended = true;
            return Ok(None);
        }
        if h.iter().all(|&b| b == 0) {
            s.ended = true;
            return Ok(None);
        }
        let want = octal(&h[148..156]).ok_or_else(|| bad("bad tar header checksum field"))?;
        let sum: u64 = h.iter().enumerate().map(|(i, &b)| if (148..156).contains(&i) { 32 } else { b as u64 }).sum();
        if sum != want {
            s.ended = true;
            return Err(bad("tar header checksum mismatch (corrupt archive); stopped reading here"));
        }
        let mut size = octal(&h[124..136]).ok_or_else(|| bad("bad tar size field"))?;
        let flag = h[156];
        let padn = (512 - size % 512) % 512;
        match flag {
            b'L' => {
                let d = read_exact_vec(&mut *s.r, size, 65536)?;
                discard(&mut *s.r, padn)?;
                long_name = Some(cstr(&d));
                continue;
            }
            b'x' => {
                let d = read_exact_vec(&mut *s.r, size, 1 << 20)?;
                discard(&mut *s.r, padn)?;
                let text = String::from_utf8_lossy(&d).to_string();
                for rec in text.split('\n') {
                    if let Some((_, kv)) = rec.split_once(' ') {
                        if let Some((k, v)) = kv.split_once('=') {
                            match k {
                                "path" => pax_path = Some(v.to_string()),
                                "size" => pax_size = v.parse().ok(),
                                _ => {}
                            }
                        }
                    }
                }
                continue;
            }
            b'g' | b'K' => {
                discard(&mut *s.r, size + padn)?;
                continue;
            }
            _ => {}
        }
        if let Some(ps) = pax_size {
            size = ps;
        }
        let mut name = cstr(&h[0..100]);
        if &h[257..262] == b"ustar" {
            let prefix = cstr(&h[345..500]);
            if !prefix.is_empty() {
                name = format!("{}/{}", prefix, name);
            }
        }
        if let Some(n) = pax_path.take().or(long_name.take()) {
            name = n;
        }
        let is_file = flag == b'0' || flag == 0 || flag == b'7';
        s.remaining = size;
        s.pad = (512 - size % 512) % 512;
        return Ok(Some(TarEntry { name, size, is_file }));
    }
}

/// Reader over the data of the entry most recently returned by `tar_next`.
pub struct TarData {
    pub st: SharedTar,
}

impl Read for TarData {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        let mut s = self.st.lock().unwrap();
        if s.remaining == 0 || b.is_empty() {
            return Ok(0);
        }
        let want = (b.len() as u64).min(s.remaining) as usize;
        let n = s.r.read(&mut b[..want])?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "tar entry is truncated"));
        }
        s.remaining -= n as u64;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("mnx_arch_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn zip_stored_and_deflated() {
        let d = tmpdir("zip");
        std::fs::write(d.join("a.jsonl"), "{\"x\":1}\n".repeat(5000)).unwrap();
        std::fs::write(d.join("b.txt"), "hello zip").unwrap();
        let st = Command::new("zip").current_dir(&d).args(["-q", "t.zip", "a.jsonl", "b.txt"]).status();
        if st.is_err() || !d.join("t.zip").exists() {
            return; // zip tool unavailable
        }
        let mut f = std::fs::File::open(d.join("t.zip")).unwrap();
        let es = zip_entries(&mut f, 100).unwrap();
        assert_eq!(es.len(), 2);
        let count = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut out = Vec::new();
        zip_open(&d.join("t.zip"), &es[0], 1 << 26, count.clone()).unwrap().read_to_end(&mut out).unwrap();
        assert_eq!(out.len(), 8 * 5000);
        let mut out = Vec::new();
        zip_open(&d.join("t.zip"), &es[1], 1 << 26, count).unwrap().read_to_end(&mut out).unwrap();
        assert_eq!(out, b"hello zip");
        // corrupt a byte inside the compressed data -> CRC / inflate error
        let mut raw = std::fs::read(d.join("t.zip")).unwrap();
        let o = es[0].offset as usize;
        let data_at = o + 30 + u16le(&raw, o + 26) as usize + u16le(&raw, o + 28) as usize;
        raw[data_at + 3] ^= 0xFF;
        std::fs::write(d.join("bad.zip"), &raw).unwrap();
        let mut f = std::fs::File::open(d.join("bad.zip")).unwrap();
        let es = zip_entries(&mut f, 100).unwrap();
        let count = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut out = Vec::new();
        assert!(zip_open(&d.join("bad.zip"), &es[0], 1 << 26, count).and_then(|mut r| r.read_to_end(&mut out)).is_err());
        // truncated zip
        let half = &raw[..raw.len() / 2];
        std::fs::write(d.join("cut.zip"), half).unwrap();
        let mut f = std::fs::File::open(d.join("cut.zip")).unwrap();
        assert!(zip_entries(&mut f, 100).is_err());
    }

    #[test]
    fn tar_listing_and_data() {
        let d = tmpdir("tar");
        std::fs::write(d.join("one.txt"), "first file").unwrap();
        std::fs::create_dir_all(d.join("sub")).unwrap();
        let long = "x".repeat(130);
        std::fs::write(d.join("sub").join(format!("{}.txt", long)), "second").unwrap();
        let st = Command::new("tar").current_dir(&d).args(["-cf", "t.tar", "one.txt", "sub"]).status();
        if st.is_err() {
            return;
        }
        let f = std::fs::File::open(d.join("t.tar")).unwrap();
        let t = tar_new(Box::new(BufReader::new(f)));
        let mut seen = Vec::new();
        while let Some(e) = tar_next(&t).unwrap() {
            let mut data = Vec::new();
            if e.is_file {
                TarData { st: t.clone() }.read_to_end(&mut data).unwrap();
            }
            seen.push((e.name, data));
        }
        assert!(seen.iter().any(|(n, d)| n == "one.txt" && d == b"first file"));
        assert!(seen.iter().any(|(n, d)| n.ends_with(&format!("{}.txt", "x".repeat(130))) && d == b"second"));
    }

    #[test]
    fn sniffing() {
        assert_eq!(sniff(b"PK\x03\x04abc"), Kind::Zip);
        assert_eq!(sniff(&[0x1f, 0x8b, 8]), Kind::Gzip);
        assert_eq!(sniff(&[0x28, 0xB5, 0x2F, 0xFD, 0]), Kind::Zstd);
        assert_eq!(sniff(b"PAR1xxxx"), Kind::Parquet);
        assert_eq!(sniff(b"{\"a\":1}"), Kind::Plain);
        assert_eq!(leaf_ext("data.jsonl.gz"), "jsonl");
        assert_eq!(leaf_ext("x.CSV"), "csv");
        assert_eq!(leaf_ext("noext"), "");
    }
}
