//! Checksums: CRC-32 (zip/gzip), SHA-256 (file integrity), XXH64 (zstd frames). Pure std, streaming.

pub struct Crc32 {
    table: [u32; 256],
    state: u32,
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc32 {
    pub fn new() -> Self {
        let mut table = [0u32; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *slot = c;
        }
        Crc32 { table, state: 0xFFFF_FFFF }
    }
    pub fn update(&mut self, data: &[u8]) {
        let mut s = self.state;
        for &b in data {
            s = self.table[((s ^ b as u32) & 0xFF) as usize] ^ (s >> 8);
        }
        self.state = s;
    }
    pub fn finish(&self) -> u32 {
        !self.state
    }
}

const K256: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
    0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
    0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

pub struct Sha256 {
    h: [u32; 8],
    buf: [u8; 64],
    fill: usize,
    total: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        Sha256 { h: [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19], buf: [0; 64], fill: 0, total: 0 }
    }
    fn block(h: &mut [u32; 8], b: &[u8]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut bb, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K256[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & bb) ^ (a & c) ^ (bb & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = bb;
            bb = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, bb, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    pub fn update(&mut self, mut data: &[u8]) {
        self.total += data.len() as u64;
        if self.fill > 0 {
            let take = (64 - self.fill).min(data.len());
            self.buf[self.fill..self.fill + take].copy_from_slice(&data[..take]);
            self.fill += take;
            data = &data[take..];
            if self.fill == 64 {
                let b = self.buf;
                Self::block(&mut self.h, &b);
                self.fill = 0;
            }
        }
        while data.len() >= 64 {
            Self::block(&mut self.h, &data[..64]);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.fill = data.len();
        }
    }
    pub fn finish_hex(mut self) -> String {
        let bits = self.total * 8;
        let mut pad = vec![0x80u8];
        while (self.fill + pad.len()) % 64 != 56 {
            pad.push(0);
        }
        pad.extend_from_slice(&bits.to_be_bytes());
        let keep = self.total;
        self.update(&pad);
        self.total = keep;
        self.h.iter().map(|x| format!("{:08x}", x)).collect()
    }
}

const P1: u64 = 11400714785074694791;
const P2: u64 = 14029467366897019727;
const P3: u64 = 1609587929392839161;
const P4: u64 = 9650029242287828579;
const P5: u64 = 2870177450012600261;

/// Streaming XXH64 (seed 0), as used by the zstd frame checksum.
pub struct Xxh64 {
    v: [u64; 4],
    buf: [u8; 32],
    fill: usize,
    total: u64,
}

impl Default for Xxh64 {
    fn default() -> Self {
        Self::new()
    }
}

fn round(acc: u64, input: u64) -> u64 {
    acc.wrapping_add(input.wrapping_mul(P2)).rotate_left(31).wrapping_mul(P1)
}
fn merge(acc: u64, v: u64) -> u64 {
    (acc ^ round(0, v)).wrapping_mul(P1).wrapping_add(P4)
}
fn rd64(b: &[u8]) -> u64 {
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

impl Xxh64 {
    pub fn new() -> Self {
        Xxh64 { v: [P1.wrapping_add(P2), P2, 0, 0u64.wrapping_sub(P1)], buf: [0; 32], fill: 0, total: 0 }
    }
    fn stripe(v: &mut [u64; 4], b: &[u8]) {
        for i in 0..4 {
            v[i] = round(v[i], rd64(&b[i * 8..]));
        }
    }
    pub fn update(&mut self, mut data: &[u8]) {
        self.total += data.len() as u64;
        if self.fill > 0 {
            let take = (32 - self.fill).min(data.len());
            self.buf[self.fill..self.fill + take].copy_from_slice(&data[..take]);
            self.fill += take;
            data = &data[take..];
            if self.fill == 32 {
                let b = self.buf;
                Self::stripe(&mut self.v, &b);
                self.fill = 0;
            }
        }
        while data.len() >= 32 {
            Self::stripe(&mut self.v, &data[..32]);
            data = &data[32..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.fill = data.len();
        }
    }
    pub fn finish(&self) -> u64 {
        let mut h = if self.total >= 32 {
            let mut h = self.v[0].rotate_left(1).wrapping_add(self.v[1].rotate_left(7)).wrapping_add(self.v[2].rotate_left(12)).wrapping_add(self.v[3].rotate_left(18));
            for i in 0..4 {
                h = merge(h, self.v[i]);
            }
            h
        } else {
            P5
        };
        h = h.wrapping_add(self.total);
        let t = &self.buf[..self.fill];
        let mut i = 0;
        while i + 8 <= t.len() {
            h = (h ^ round(0, rd64(&t[i..]))).rotate_left(27).wrapping_mul(P1).wrapping_add(P4);
            i += 8;
        }
        if i + 4 <= t.len() {
            let k = u32::from_le_bytes([t[i], t[i + 1], t[i + 2], t[i + 3]]) as u64;
            h = (h ^ k.wrapping_mul(P1)).rotate_left(23).wrapping_mul(P2).wrapping_add(P3);
            i += 4;
        }
        while i < t.len() {
            h = (h ^ (t[i] as u64).wrapping_mul(P5)).rotate_left(11).wrapping_mul(P1);
            i += 1;
        }
        h ^= h >> 33;
        h = h.wrapping_mul(P2);
        h ^= h >> 29;
        h = h.wrapping_mul(P3);
        h ^ (h >> 32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        let mut c = Crc32::new();
        c.update(b"123456789");
        assert_eq!(c.finish(), 0xCBF43926);
        let mut s = Sha256::new();
        s.update(b"abc");
        assert_eq!(s.finish_hex(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let mut s = Sha256::new();
        for _ in 0..1000 {
            s.update(b"aaaaaaaaaa");
        }
        assert_eq!(s.finish_hex(), "27dd1f61b867b6a0f6e9d8a41c43231de52107e53ae424de8f847b821db4b711");
        let mut x = Xxh64::new();
        x.update(b"");
        assert_eq!(x.finish(), 0xEF46DB3751D8E999);
    }
}
