//! From-scratch cryptographic primitives for the TLS 1.3 client (no_std, no
//! external crates). Everything here is a faithful port of a public reference:
//!   - SHA-256              FIPS 180-4
//!   - HMAC / HKDF          RFC 2104 / RFC 5869
//!   - HKDF-Expand-Label    RFC 8446 §7.1 (TLS 1.3 key schedule)
//!   - X25519               RFC 7748 (ported from TweetNaCl, public domain)
//!   - ChaCha20 / Poly1305  RFC 8439 (Poly1305 ported from poly1305-donna)
//!
//! Constant-time-ness is best-effort, not audited. This is a hobby-OS TLS
//! stack: it gives us confidentiality against a passive network, not the full
//! security guarantees of a hardened library. Certificate validation is NOT
//! performed (no CA store, no wall clock) — see tls.rs for the honest caveat.

// ───────────────────────────── SHA-256 ──────────────────────────────────────

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1,
    0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
    0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
    0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
    0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
    0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

#[derive(Clone)]
pub struct Sha256 {
    h: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total: u64,
}

impl Sha256 {
    pub fn new() -> Self {
        Sha256 {
            h: [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19],
            buf: [0; 64],
            buf_len: 0,
            total: 0,
        }
    }

    fn block(&mut self, blk: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([blk[4*i], blk[4*i+1], blk[4*i+2], blk[4*i+3]]);
        }
        for i in 16..64 {
            let s0 = w[i-15].rotate_right(7) ^ w[i-15].rotate_right(18) ^ (w[i-15] >> 3);
            let s1 = w[i-2].rotate_right(17) ^ w[i-2].rotate_right(19) ^ (w[i-2] >> 10);
            w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
        }
        let mut a = self.h[0]; let mut b = self.h[1]; let mut c = self.h[2];
        let mut d = self.h[3]; let mut e = self.h[4]; let mut f = self.h[5];
        let mut g = self.h[6]; let mut hh = self.h[7];
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(SHA256_K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g; g = f; f = e; e = d.wrapping_add(t1);
            d = c; c = b; b = a; a = t1.wrapping_add(t2);
        }
        self.h[0] = self.h[0].wrapping_add(a); self.h[1] = self.h[1].wrapping_add(b);
        self.h[2] = self.h[2].wrapping_add(c); self.h[3] = self.h[3].wrapping_add(d);
        self.h[4] = self.h[4].wrapping_add(e); self.h[5] = self.h[5].wrapping_add(f);
        self.h[6] = self.h[6].wrapping_add(g); self.h[7] = self.h[7].wrapping_add(hh);
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total = self.total.wrapping_add(data.len() as u64);
        if self.buf_len > 0 {
            let need = 64 - self.buf_len;
            let take = need.min(data.len());
            self.buf[self.buf_len..self.buf_len+take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let blk = self.buf;
                self.block(&blk);
                self.buf_len = 0;
            }
        }
        while data.len() >= 64 {
            let mut blk = [0u8; 64];
            blk.copy_from_slice(&data[..64]);
            self.block(&blk);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    pub fn finalize(mut self) -> [u8; 32] {
        let bits = self.total.wrapping_mul(8);
        let mut i = self.buf_len;
        self.buf[i] = 0x80; i += 1;
        if i > 56 {
            while i < 64 { self.buf[i] = 0; i += 1; }
            let blk = self.buf; self.block(&blk);
            i = 0;
        }
        while i < 56 { self.buf[i] = 0; i += 1; }
        self.buf[56..64].copy_from_slice(&bits.to_be_bytes());
        let blk = self.buf; self.block(&blk);
        let mut out = [0u8; 32];
        for j in 0..8 { out[4*j..4*j+4].copy_from_slice(&self.h[j].to_be_bytes()); }
        out
    }
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut s = Sha256::new();
    s.update(data);
    s.finalize()
}

// ──────────────────────── HMAC-SHA256 / HKDF ────────────────────────────────

pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        let kh = sha256(key);
        k[..32].copy_from_slice(&kh);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 { ipad[i] ^= k[i]; opad[i] ^= k[i]; }
    let mut inner = Sha256::new();
    inner.update(&ipad);
    inner.update(msg);
    let ih = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(&opad);
    outer.update(&ih);
    outer.finalize()
}

pub fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {
    hmac_sha256(salt, ikm)
}

/// HKDF-Expand to `out.len()` bytes (out.len() <= 255*32).
pub fn hkdf_expand(prk: &[u8], info: &[u8], out: &mut [u8]) {
    let mut t = [0u8; 32];
    let mut t_len = 0usize;
    let mut filled = 0usize;
    let mut counter = 1u8;
    while filled < out.len() {
        let mut m = Sha256::new();
        // HMAC needs full message; build T(n) = HMAC(prk, T(n-1) | info | n)
        // We compute HMAC via hmac_sha256 over a concatenation buffer.
        // Bound: info <= ~512 here; use a fixed scratch.
        let mut scratch = [0u8; 32 + 600 + 1];
        let mut sl = 0;
        if t_len > 0 { scratch[..t_len].copy_from_slice(&t[..t_len]); sl += t_len; }
        let il = info.len().min(600);
        scratch[sl..sl+il].copy_from_slice(&info[..il]); sl += il;
        scratch[sl] = counter; sl += 1;
        let _ = &mut m; // (kept for clarity; not used directly)
        t = hmac_sha256(prk, &scratch[..sl]);
        t_len = 32;
        let take = (out.len() - filled).min(32);
        out[filled..filled+take].copy_from_slice(&t[..take]);
        filled += take;
        counter = counter.wrapping_add(1);
    }
}

/// TLS 1.3 HKDF-Expand-Label (RFC 8446 §7.1).
/// HkdfLabel = uint16 length | "tls13 "+label (opaque<7..255>) | context (opaque<0..255>)
pub fn hkdf_expand_label(secret: &[u8], label: &str, context: &[u8], out: &mut [u8]) {
    let mut info = [0u8; 600];
    let mut n = 0;
    let outlen = out.len() as u16;
    info[0] = (outlen >> 8) as u8; info[1] = outlen as u8; n += 2;
    let full_label_len = 6 + label.len(); // "tls13 " prefix
    info[n] = full_label_len as u8; n += 1;
    info[n..n+6].copy_from_slice(b"tls13 "); n += 6;
    info[n..n+label.len()].copy_from_slice(label.as_bytes()); n += label.len();
    info[n] = context.len() as u8; n += 1;
    info[n..n+context.len()].copy_from_slice(context); n += context.len();
    hkdf_expand(secret, &info[..n], out);
}

/// Derive-Secret(Secret, Label, Messages) = HKDF-Expand-Label(Secret, Label, Hash(Messages), Hash.length)
pub fn derive_secret(secret: &[u8], label: &str, transcript_hash: &[u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    hkdf_expand_label(secret, label, transcript_hash, &mut out);
    out
}

// ───────────────────────────── ChaCha20 ─────────────────────────────────────

#[inline]
fn qr(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]); s[d] ^= s[a]; s[d] = s[d].rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]); s[b] ^= s[c]; s[b] = s[b].rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]); s[d] ^= s[a]; s[d] = s[d].rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]); s[b] ^= s[c]; s[b] = s[b].rotate_left(7);
}

fn chacha20_block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut s = [0u32; 16];
    s[0] = 0x61707865; s[1] = 0x3320646e; s[2] = 0x79622d32; s[3] = 0x6b206574;
    for i in 0..8 { s[4+i] = u32::from_le_bytes([key[4*i], key[4*i+1], key[4*i+2], key[4*i+3]]); }
    s[12] = counter;
    for i in 0..3 { s[13+i] = u32::from_le_bytes([nonce[4*i], nonce[4*i+1], nonce[4*i+2], nonce[4*i+3]]); }
    let mut x = s;
    for _ in 0..10 {
        qr(&mut x, 0, 4, 8, 12); qr(&mut x, 1, 5, 9, 13);
        qr(&mut x, 2, 6, 10, 14); qr(&mut x, 3, 7, 11, 15);
        qr(&mut x, 0, 5, 10, 15); qr(&mut x, 1, 6, 11, 12);
        qr(&mut x, 2, 7, 8, 13); qr(&mut x, 3, 4, 9, 14);
    }
    let mut out = [0u8; 64];
    for i in 0..16 {
        let v = x[i].wrapping_add(s[i]);
        out[4*i..4*i+4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

/// XOR `data` in place with the ChaCha20 keystream starting at `counter`.
pub fn chacha20_xor(key: &[u8; 32], counter: u32, nonce: &[u8; 12], data: &mut [u8]) {
    let mut ctr = counter;
    let mut off = 0;
    while off < data.len() {
        let ks = chacha20_block(key, ctr, nonce);
        let n = (data.len() - off).min(64);
        for i in 0..n { data[off+i] ^= ks[i]; }
        off += n;
        ctr = ctr.wrapping_add(1);
    }
}

// ───────────────────────────── Poly1305 ─────────────────────────────────────
// Ported from poly1305-donna (32-bit), public domain.

struct Poly1305 {
    r: [u32; 5],
    h: [u32; 5],
    pad: [u32; 4],
    leftover: usize,
    buffer: [u8; 16],
    finished: bool,
}

impl Poly1305 {
    fn new(key: &[u8; 32]) -> Self {
        let rd = |i: usize| u32::from_le_bytes([key[i], key[i+1], key[i+2], key[i+3]]);
        let t0 = rd(0); let t1 = rd(4); let t2 = rd(8); let t3 = rd(12);
        let r = [
            t0 & 0x3ffffff,
            ((t0 >> 26) | (t1 << 6)) & 0x3ffff03,
            ((t1 >> 20) | (t2 << 12)) & 0x3ffc0ff,
            ((t2 >> 14) | (t3 << 18)) & 0x3f03fff,
            (t3 >> 8) & 0x00fffff,
        ];
        let pad = [rd(16), rd(20), rd(24), rd(28)];
        Poly1305 { r, h: [0; 5], pad, leftover: 0, buffer: [0; 16], finished: false }
    }

    fn blocks(&mut self, mut m: &[u8], full: bool) {
        let hibit: u32 = if full { 1 << 24 } else { 0 };
        let r0 = self.r[0]; let r1 = self.r[1]; let r2 = self.r[2];
        let r3 = self.r[3]; let r4 = self.r[4];
        let s1 = r1 * 5; let s2 = r2 * 5; let s3 = r3 * 5; let s4 = r4 * 5;
        let mut h0 = self.h[0]; let mut h1 = self.h[1]; let mut h2 = self.h[2];
        let mut h3 = self.h[3]; let mut h4 = self.h[4];
        while m.len() >= 16 {
            let rd = |i: usize| u32::from_le_bytes([m[i], m[i+1], m[i+2], m[i+3]]);
            let t0 = rd(0); let t1 = rd(4); let t2 = rd(8); let t3 = rd(12);
            h0 += t0 & 0x3ffffff;
            h1 += ((t0 >> 26) | (t1 << 6)) & 0x3ffffff;
            h2 += ((t1 >> 20) | (t2 << 12)) & 0x3ffffff;
            h3 += ((t2 >> 14) | (t3 << 18)) & 0x3ffffff;
            h4 += (t3 >> 8) | hibit;

            let d0 = (h0 as u64) * (r0 as u64) + (h1 as u64) * (s4 as u64)
                + (h2 as u64) * (s3 as u64) + (h3 as u64) * (s2 as u64) + (h4 as u64) * (s1 as u64);
            let mut d1 = (h0 as u64) * (r1 as u64) + (h1 as u64) * (r0 as u64)
                + (h2 as u64) * (s4 as u64) + (h3 as u64) * (s3 as u64) + (h4 as u64) * (s2 as u64);
            let mut d2 = (h0 as u64) * (r2 as u64) + (h1 as u64) * (r1 as u64)
                + (h2 as u64) * (r0 as u64) + (h3 as u64) * (s4 as u64) + (h4 as u64) * (s3 as u64);
            let mut d3 = (h0 as u64) * (r3 as u64) + (h1 as u64) * (r2 as u64)
                + (h2 as u64) * (r1 as u64) + (h3 as u64) * (r0 as u64) + (h4 as u64) * (s4 as u64);
            let mut d4 = (h0 as u64) * (r4 as u64) + (h1 as u64) * (r3 as u64)
                + (h2 as u64) * (r2 as u64) + (h3 as u64) * (r1 as u64) + (h4 as u64) * (r0 as u64);

            let mut c: u32;
            c = (d0 >> 26) as u32; h0 = (d0 as u32) & 0x3ffffff;
            d1 += c as u64; c = (d1 >> 26) as u32; h1 = (d1 as u32) & 0x3ffffff;
            d2 += c as u64; c = (d2 >> 26) as u32; h2 = (d2 as u32) & 0x3ffffff;
            d3 += c as u64; c = (d3 >> 26) as u32; h3 = (d3 as u32) & 0x3ffffff;
            d4 += c as u64; c = (d4 >> 26) as u32; h4 = (d4 as u32) & 0x3ffffff;
            h0 += c * 5; c = h0 >> 26; h0 &= 0x3ffffff; h1 += c;

            m = &m[16..];
        }
        self.h = [h0, h1, h2, h3, h4];
    }

    fn update(&mut self, mut data: &[u8]) {
        if self.leftover > 0 {
            let want = (16 - self.leftover).min(data.len());
            for i in 0..want { self.buffer[self.leftover + i] = data[i]; }
            self.leftover += want;
            data = &data[want..];
            if self.leftover < 16 { return; }
            let buf = self.buffer;
            self.blocks(&buf, true);
            self.leftover = 0;
        }
        if data.len() >= 16 {
            let n = data.len() & !15;
            self.blocks(&data[..n], true);
            data = &data[n..];
        }
        if !data.is_empty() {
            for i in 0..data.len() { self.buffer[i] = data[i]; }
            self.leftover = data.len();
        }
    }

    fn finalize(mut self) -> [u8; 16] {
        if self.leftover > 0 {
            self.buffer[self.leftover] = 1;
            for i in self.leftover+1..16 { self.buffer[i] = 0; }
            self.finished = true;
            let buf = self.buffer;
            self.blocks(&buf, false);
        }
        let mut h0 = self.h[0]; let mut h1 = self.h[1]; let mut h2 = self.h[2];
        let mut h3 = self.h[3]; let mut h4 = self.h[4];
        let mut c: u32;
        c = h1 >> 26; h1 &= 0x3ffffff; h2 += c;
        c = h2 >> 26; h2 &= 0x3ffffff; h3 += c;
        c = h3 >> 26; h3 &= 0x3ffffff; h4 += c;
        c = h4 >> 26; h4 &= 0x3ffffff; h0 += c * 5;
        c = h0 >> 26; h0 &= 0x3ffffff; h1 += c;

        let mut g0 = h0.wrapping_add(5); c = g0 >> 26; g0 &= 0x3ffffff;
        let mut g1 = h1.wrapping_add(c); c = g1 >> 26; g1 &= 0x3ffffff;
        let mut g2 = h2.wrapping_add(c); c = g2 >> 26; g2 &= 0x3ffffff;
        let mut g3 = h3.wrapping_add(c); c = g3 >> 26; g3 &= 0x3ffffff;
        let g4 = h4.wrapping_add(c).wrapping_sub(1 << 26);

        let mask = (g4 >> 31).wrapping_sub(1); // if g4 negative, mask=0
        g0 &= mask; g1 &= mask; g2 &= mask; g3 &= mask;
        let g4m = g4 & mask;
        let nmask = !mask;
        h0 = (h0 & nmask) | g0; h1 = (h1 & nmask) | g1; h2 = (h2 & nmask) | g2;
        h3 = (h3 & nmask) | g3; h4 = (h4 & nmask) | g4m;

        // h = h % (2^128)
        let f0 = (h0 | (h1 << 26)) as u64 + self.pad[0] as u64;
        let f1 = ((h1 >> 6) | (h2 << 20)) as u64 + self.pad[1] as u64;
        let f2 = ((h2 >> 12) | (h3 << 14)) as u64 + self.pad[2] as u64;
        let f3 = ((h3 >> 18) | (h4 << 8)) as u64 + self.pad[3] as u64;
        let mut out = [0u8; 16];
        let mut f = f0;
        out[0..4].copy_from_slice(&((f as u32).to_le_bytes()));
        f = (f >> 32) + f1;
        out[4..8].copy_from_slice(&((f as u32).to_le_bytes()));
        f = (f >> 32) + f2;
        out[8..12].copy_from_slice(&((f as u32).to_le_bytes()));
        f = (f >> 32) + f3;
        out[12..16].copy_from_slice(&((f as u32).to_le_bytes()));
        out
    }
}

// ───────────────────── AEAD_CHACHA20_POLY1305 (RFC 8439) ─────────────────────

fn poly1305_key(key: &[u8; 32], nonce: &[u8; 12]) -> [u8; 32] {
    let blk = chacha20_block(key, 0, nonce);
    let mut k = [0u8; 32];
    k.copy_from_slice(&blk[..32]);
    k
}

fn poly1305_tag(otk: &[u8; 32], aad: &[u8], ct: &[u8]) -> [u8; 16] {
    let mut p = Poly1305::new(otk);
    p.update(aad);
    let apad = (16 - (aad.len() % 16)) % 16;
    if apad > 0 { p.update(&[0u8; 16][..apad]); }
    p.update(ct);
    let cpad = (16 - (ct.len() % 16)) % 16;
    if cpad > 0 { p.update(&[0u8; 16][..cpad]); }
    let mut lens = [0u8; 16];
    lens[..8].copy_from_slice(&(aad.len() as u64).to_le_bytes());
    lens[8..].copy_from_slice(&(ct.len() as u64).to_le_bytes());
    p.update(&lens);
    p.finalize()
}

/// Seal: encrypt `buf` in place and return the 16-byte tag.
pub fn aead_seal(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], buf: &mut [u8]) -> [u8; 16] {
    let otk = poly1305_key(key, nonce);
    chacha20_xor(key, 1, nonce, buf);
    poly1305_tag(&otk, aad, buf)
}

/// Open: verify `tag` and decrypt `buf` in place. Returns false on auth failure.
pub fn aead_open(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], buf: &mut [u8], tag: &[u8; 16]) -> bool {
    let otk = poly1305_key(key, nonce);
    let expect = poly1305_tag(&otk, aad, buf);
    let mut diff = 0u8;
    for i in 0..16 { diff |= expect[i] ^ tag[i]; }
    if diff != 0 { return false; }
    chacha20_xor(key, 1, nonce, buf);
    true
}

// ───────────────────────────── X25519 ───────────────────────────────────────
// Ported from TweetNaCl (D. J. Bernstein et al.), public domain.

type Gf = [i64; 16];

fn gf0() -> Gf { [0; 16] }

fn car25519(o: &mut Gf) {
    for i in 0..16 {
        o[i] += 1 << 16;
        let c = o[i] >> 16;
        if i < 15 {
            o[i+1] += c - 1;
        } else {
            o[0] += 38 * (c - 1);
        }
        o[i] -= c << 16;
    }
}

fn sel25519(p: &mut Gf, q: &mut Gf, b: i64) {
    let c = !(b - 1);
    for i in 0..16 {
        let t = c & (p[i] ^ q[i]);
        p[i] ^= t;
        q[i] ^= t;
    }
}

fn pack25519(o: &mut [u8; 32], n: &Gf) {
    let mut t: Gf = *n;
    car25519(&mut t); car25519(&mut t); car25519(&mut t);
    for _ in 0..2 {
        let mut m: Gf = gf0();
        m[0] = t[0] - 0xffed;
        for i in 1..15 {
            m[i] = t[i] - 0xffff - ((m[i-1] >> 16) & 1);
            m[i-1] &= 0xffff;
        }
        m[15] = t[15] - 0x7fff - ((m[14] >> 16) & 1);
        let b = (m[15] >> 16) & 1;
        m[14] &= 0xffff;
        sel25519(&mut t, &mut m, 1 - b);
    }
    for i in 0..16 {
        o[2*i] = (t[i] & 0xff) as u8;
        o[2*i+1] = (t[i] >> 8) as u8;
    }
}

fn unpack25519(o: &mut Gf, n: &[u8; 32]) {
    for i in 0..16 {
        o[i] = n[2*i] as i64 + ((n[2*i+1] as i64) << 8);
    }
    o[15] &= 0x7fff;
}

fn add_gf(o: &mut Gf, a: &Gf, b: &Gf) { for i in 0..16 { o[i] = a[i] + b[i]; } }
fn sub_gf(o: &mut Gf, a: &Gf, b: &Gf) { for i in 0..16 { o[i] = a[i] - b[i]; } }

fn mul_gf(o: &mut Gf, a: &Gf, b: &Gf) {
    let mut t = [0i64; 31];
    for i in 0..16 { for j in 0..16 { t[i+j] += a[i] * b[j]; } }
    for i in 0..15 { t[i] += 38 * t[i+16]; }
    let mut r: Gf = gf0();
    r[..16].copy_from_slice(&t[..16]);
    car25519(&mut r); car25519(&mut r);
    *o = r;
}

fn sq_gf(o: &mut Gf, a: &Gf) { let ac = *a; mul_gf(o, &ac, &ac); }

fn inv25519(o: &mut Gf, i: &Gf) {
    let mut c: Gf = *i;
    let mut a = 253i32;
    while a >= 0 {
        let cc = c;
        sq_gf(&mut c, &cc);
        if a != 2 && a != 4 {
            let cc2 = c;
            mul_gf(&mut c, &cc2, i);
        }
        a -= 1;
    }
    *o = c;
}

/// X25519 scalar multiplication: q = n * p (RFC 7748).
pub fn x25519(q: &mut [u8; 32], n: &[u8; 32], p: &[u8; 32]) {
    let mut z = *n;
    z[31] = (z[31] & 127) | 64;
    z[0] &= 248;
    let mut x: Gf = gf0();
    unpack25519(&mut x, p);
    let mut a: Gf = gf0();
    let mut b: Gf = x;
    let mut c: Gf = gf0();
    let mut d: Gf = gf0();
    let mut e: Gf;
    let mut f: Gf;
    a[0] = 1; d[0] = 1;
    let mut i = 254i32;
    while i >= 0 {
        let r = ((z[(i >> 3) as usize] >> (i & 7)) & 1) as i64;
        sel25519(&mut a, &mut b, r);
        sel25519(&mut c, &mut d, r);
        e = gf0(); add_gf(&mut e, &a, &c);
        { let aa = a; sub_gf(&mut a, &aa, &c); }
        { let cc = c; add_gf(&mut c, &b, &d); let _ = cc; }
        { let bb = b; sub_gf(&mut b, &bb, &d); let _ = bb; }
        d = gf0(); sq_gf(&mut d, &e);
        f = gf0(); sq_gf(&mut f, &a);
        { let cc = c; let aa = a; mul_gf(&mut a, &cc, &aa); }
        { let ee = e; mul_gf(&mut c, &b, &ee); }
        e = gf0(); add_gf(&mut e, &a, &c);
        { let aa = a; sub_gf(&mut a, &aa, &c); }
        { let aa = a; sq_gf(&mut b, &aa); }
        { let dd = d; sub_gf(&mut c, &dd, &f); }
        let _121665: Gf = { let mut g = gf0(); g[0] = 0xDB41; g[1] = 1; g };
        { let cc = c; mul_gf(&mut a, &cc, &_121665); }
        { let aa = a; add_gf(&mut a, &aa, &d); }
        { let cc = c; mul_gf(&mut c, &cc, &a); }
        { let dd = d; mul_gf(&mut a, &dd, &f); }
        { let bb = b; mul_gf(&mut d, &bb, &x); }
        { let ee = e; sq_gf(&mut b, &ee); }
        sel25519(&mut a, &mut b, r);
        sel25519(&mut c, &mut d, r);
        i -= 1;
    }
    // x32 = inv(c); x16 = a * x32; q = pack(x16)
    let mut cinv: Gf = gf0();
    inv25519(&mut cinv, &c);
    let mut res: Gf = gf0();
    mul_gf(&mut res, &a, &cinv);
    pack25519(q, &res);
}

/// Compute the X25519 public key for a clamped private scalar (base point 9).
pub fn x25519_base(pubkey: &mut [u8; 32], private: &[u8; 32]) {
    let mut base = [0u8; 32];
    base[0] = 9;
    x25519(pubkey, private, &base);
}

// ───────────────────────────── Self-test ────────────────────────────────────
// Verified at boot against RFC test vectors; logs to serial.

fn hex_eq(a: &[u8], hex: &str) -> bool {
    if a.len() * 2 != hex.len() { return false; }
    let hb = hex.as_bytes();
    for i in 0..a.len() {
        let hi = hexval(hb[2*i]); let lo = hexval(hb[2*i+1]);
        if (hi << 4) | lo != a[i] { return false; }
    }
    true
}
fn hexval(c: u8) -> u8 {
    match c { b'0'..=b'9' => c - b'0', b'a'..=b'f' => c - b'a' + 10, _ => 0 }
}

pub fn selftest() -> bool {
    let mut ok = true;
    // SHA-256("abc")
    let h = sha256(b"abc");
    if !hex_eq(&h, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad") {
        crate::serial::write_str("  [crypto] SHA-256 FAIL\n"); ok = false;
    }
    // X25519 RFC 7748 §6.1 — Alice priv/pub
    let mut apriv = [0u8; 32];
    let h2 = "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a";
    for i in 0..32 { apriv[i] = (hexval(h2.as_bytes()[2*i]) << 4) | hexval(h2.as_bytes()[2*i+1]); }
    let mut apub = [0u8; 32];
    x25519_base(&mut apub, &apriv);
    if !hex_eq(&apub, "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a") {
        crate::serial::write_str("  [crypto] X25519 FAIL\n"); ok = false;
    }
    // ChaCha20-Poly1305 RFC 8439 §2.8.2 AEAD vector
    let mut key = [0u8; 32];
    for i in 0..32 { key[i] = (0x80 + i) as u8; }
    let nonce: [u8; 12] = [0x07,0,0,0,0x40,0x41,0x42,0x43,0x44,0x45,0x46,0x47];
    let aad: [u8; 12] = [0x50,0x51,0x52,0x53,0xc0,0xc1,0xc2,0xc3,0xc4,0xc5,0xc6,0xc7];
    let mut pt = *b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
    let tag = aead_seal(&key, &nonce, &aad, &mut pt[..]);
    if !hex_eq(&tag, "1ae10b594f09e26a7e902ecbd0600691") {
        crate::serial::write_str("  [crypto] ChaCha20-Poly1305 FAIL\n"); ok = false;
    }
    // HKDF RFC 5869 Test Case 1
    let ikm = [0x0bu8; 22];
    let salt: [u8; 13] = [0,1,2,3,4,5,6,7,8,9,10,11,12];
    let info: [u8; 10] = [0xf0,0xf1,0xf2,0xf3,0xf4,0xf5,0xf6,0xf7,0xf8,0xf9];
    let prk = hkdf_extract(&salt, &ikm);
    if !hex_eq(&prk, "077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5") {
        crate::serial::write_str("  [crypto] HKDF-Extract FAIL\n"); ok = false;
    }
    let mut okm = [0u8; 42];
    hkdf_expand(&prk, &info, &mut okm);
    if !hex_eq(&okm, "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865") {
        crate::serial::write_str("  [crypto] HKDF-Expand FAIL\n"); ok = false;
    }
    if ok { crate::serial::write_str("  [crypto] self-test OK (SHA-256, X25519, ChaCha20-Poly1305, HKDF)\n"); }
    ok
}
