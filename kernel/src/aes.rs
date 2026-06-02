//! AES-128 (FIPS-197) + AES Key Unwrap (RFC 3394) — the block cipher WPA2 needs
//! for the GTK unwrap in the 4-way handshake and for the CCMP data cipher. Pure
//! `core`, no alloc, host-testable; verified against the FIPS-197 and RFC 3394
//! known-answer vectors. A faithful port of the reference, not a hardened/constant-
//! time implementation (this is a hobby-OS WiFi stack).

#![allow(dead_code)]

// ───────────────────────────── S-boxes ──────────────────────────────────────

static SBOX: [u8; 256] = [
    0x63,0x7c,0x77,0x7b,0xf2,0x6b,0x6f,0xc5,0x30,0x01,0x67,0x2b,0xfe,0xd7,0xab,0x76,
    0xca,0x82,0xc9,0x7d,0xfa,0x59,0x47,0xf0,0xad,0xd4,0xa2,0xaf,0x9c,0xa4,0x72,0xc0,
    0xb7,0xfd,0x93,0x26,0x36,0x3f,0xf7,0xcc,0x34,0xa5,0xe5,0xf1,0x71,0xd8,0x31,0x15,
    0x04,0xc7,0x23,0xc3,0x18,0x96,0x05,0x9a,0x07,0x12,0x80,0xe2,0xeb,0x27,0xb2,0x75,
    0x09,0x83,0x2c,0x1a,0x1b,0x6e,0x5a,0xa0,0x52,0x3b,0xd6,0xb3,0x29,0xe3,0x2f,0x84,
    0x53,0xd1,0x00,0xed,0x20,0xfc,0xb1,0x5b,0x6a,0xcb,0xbe,0x39,0x4a,0x4c,0x58,0xcf,
    0xd0,0xef,0xaa,0xfb,0x43,0x4d,0x33,0x85,0x45,0xf9,0x02,0x7f,0x50,0x3c,0x9f,0xa8,
    0x51,0xa3,0x40,0x8f,0x92,0x9d,0x38,0xf5,0xbc,0xb6,0xda,0x21,0x10,0xff,0xf3,0xd2,
    0xcd,0x0c,0x13,0xec,0x5f,0x97,0x44,0x17,0xc4,0xa7,0x7e,0x3d,0x64,0x5d,0x19,0x73,
    0x60,0x81,0x4f,0xdc,0x22,0x2a,0x90,0x88,0x46,0xee,0xb8,0x14,0xde,0x5e,0x0b,0xdb,
    0xe0,0x32,0x3a,0x0a,0x49,0x06,0x24,0x5c,0xc2,0xd3,0xac,0x62,0x91,0x95,0xe4,0x79,
    0xe7,0xc8,0x37,0x6d,0x8d,0xd5,0x4e,0xa9,0x6c,0x56,0xf4,0xea,0x65,0x7a,0xae,0x08,
    0xba,0x78,0x25,0x2e,0x1c,0xa6,0xb4,0xc6,0xe8,0xdd,0x74,0x1f,0x4b,0xbd,0x8b,0x8a,
    0x70,0x3e,0xb5,0x66,0x48,0x03,0xf6,0x0e,0x61,0x35,0x57,0xb9,0x86,0xc1,0x1d,0x9e,
    0xe1,0xf8,0x98,0x11,0x69,0xd9,0x8e,0x94,0x9b,0x1e,0x87,0xe9,0xce,0x55,0x28,0xdf,
    0x8c,0xa1,0x89,0x0d,0xbf,0xe6,0x42,0x68,0x41,0x99,0x2d,0x0f,0xb0,0x54,0xbb,0x16,
];

fn inv_sbox() -> [u8; 256] {
    let mut inv = [0u8; 256];
    let mut i = 0;
    while i < 256 { inv[SBOX[i] as usize] = i as u8; i += 1; }
    inv
}

const RCON: [u8; 10] = [0x01,0x02,0x04,0x08,0x10,0x20,0x40,0x80,0x1b,0x36];

// GF(2^8) multiply (xtime-based), used by MixColumns.
fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    let mut i = 0;
    while i < 8 {
        if b & 1 != 0 { p ^= a; }
        let hi = a & 0x80;
        a <<= 1;
        if hi != 0 { a ^= 0x1b; }
        b >>= 1;
        i += 1;
    }
    p
}

/// An expanded AES-128 key (11 round keys × 16 bytes).
pub struct Aes128 {
    rk: [u8; 176],
}

impl Aes128 {
    pub fn new(key: &[u8; 16]) -> Self {
        let mut rk = [0u8; 176];
        rk[..16].copy_from_slice(key);
        let mut i = 16;
        let mut rcon_i = 0;
        while i < 176 {
            let mut t = [rk[i - 4], rk[i - 3], rk[i - 2], rk[i - 1]];
            if i % 16 == 0 {
                // RotWord + SubWord + Rcon
                let tmp = t[0]; t[0] = t[1]; t[1] = t[2]; t[2] = t[3]; t[3] = tmp;
                for b in t.iter_mut() { *b = SBOX[*b as usize]; }
                t[0] ^= RCON[rcon_i];
                rcon_i += 1;
            }
            for j in 0..4 { rk[i + j] = rk[i - 16 + j] ^ t[j]; }
            i += 4;
        }
        Aes128 { rk }
    }

    fn add_round_key(state: &mut [u8; 16], rk: &[u8]) {
        for i in 0..16 { state[i] ^= rk[i]; }
    }

    pub fn encrypt_block(&self, block: &[u8; 16]) -> [u8; 16] {
        let mut s = *block;
        Self::add_round_key(&mut s, &self.rk[0..16]);
        for round in 1..10 {
            for b in s.iter_mut() { *b = SBOX[*b as usize]; }   // SubBytes
            shift_rows(&mut s);
            mix_columns(&mut s);
            Self::add_round_key(&mut s, &self.rk[round * 16..round * 16 + 16]);
        }
        for b in s.iter_mut() { *b = SBOX[*b as usize]; }
        shift_rows(&mut s);
        Self::add_round_key(&mut s, &self.rk[160..176]);
        s
    }

    pub fn decrypt_block(&self, block: &[u8; 16]) -> [u8; 16] {
        let inv = inv_sbox();
        let mut s = *block;
        Self::add_round_key(&mut s, &self.rk[160..176]);
        for round in (1..10).rev() {
            inv_shift_rows(&mut s);
            for b in s.iter_mut() { *b = inv[*b as usize]; }
            Self::add_round_key(&mut s, &self.rk[round * 16..round * 16 + 16]);
            inv_mix_columns(&mut s);
        }
        inv_shift_rows(&mut s);
        for b in s.iter_mut() { *b = inv[*b as usize]; }
        Self::add_round_key(&mut s, &self.rk[0..16]);
        s
    }
}

fn shift_rows(s: &mut [u8; 16]) {
    // state is column-major: s[r + 4c]. Rotate row r left by r.
    let t = *s;
    for r in 1..4 {
        for c in 0..4 { s[r + 4 * c] = t[r + 4 * ((c + r) % 4)]; }
    }
}
fn inv_shift_rows(s: &mut [u8; 16]) {
    let t = *s;
    for r in 1..4 {
        for c in 0..4 { s[r + 4 * c] = t[r + 4 * ((c + 4 - r) % 4)]; }
    }
}
fn mix_columns(s: &mut [u8; 16]) {
    for c in 0..4 {
        let i = 4 * c;
        let a0 = s[i]; let a1 = s[i+1]; let a2 = s[i+2]; let a3 = s[i+3];
        s[i]   = gmul(a0,2) ^ gmul(a1,3) ^ a2 ^ a3;
        s[i+1] = a0 ^ gmul(a1,2) ^ gmul(a2,3) ^ a3;
        s[i+2] = a0 ^ a1 ^ gmul(a2,2) ^ gmul(a3,3);
        s[i+3] = gmul(a0,3) ^ a1 ^ a2 ^ gmul(a3,2);
    }
}
fn inv_mix_columns(s: &mut [u8; 16]) {
    for c in 0..4 {
        let i = 4 * c;
        let a0 = s[i]; let a1 = s[i+1]; let a2 = s[i+2]; let a3 = s[i+3];
        s[i]   = gmul(a0,14) ^ gmul(a1,11) ^ gmul(a2,13) ^ gmul(a3,9);
        s[i+1] = gmul(a0,9)  ^ gmul(a1,14) ^ gmul(a2,11) ^ gmul(a3,13);
        s[i+2] = gmul(a0,13) ^ gmul(a1,9)  ^ gmul(a2,14) ^ gmul(a3,11);
        s[i+3] = gmul(a0,11) ^ gmul(a1,13) ^ gmul(a2,9)  ^ gmul(a3,14);
    }
}

// ───────────────────── AES Key Unwrap (RFC 3394) ────────────────────────────
//
// Used to recover the GTK from EAPOL message 3 (the GTK is wrapped with the KEK).
// `wrapped` is n+1 64-bit blocks (IV ‖ ciphertext); returns the n-block plaintext
// and whether the integrity check (default IV 0xA6A6A6A6A6A6A6A6) passed.

/// Unwrap `wrapped` (len = (n+1)*8) with KEK; writes n*8 bytes to `out`. Returns
/// true iff the RFC 3394 integrity check holds.
pub fn key_unwrap(kek: &[u8; 16], wrapped: &[u8], out: &mut [u8]) -> bool {
    if wrapped.len() < 16 || wrapped.len() % 8 != 0 { return false; }
    let n = wrapped.len() / 8 - 1;
    if out.len() < n * 8 { return false; }
    let aes = Aes128::new(kek);
    let mut a = [0u8; 8];
    a.copy_from_slice(&wrapped[0..8]);
    let mut r = [[0u8; 8]; 32];
    if n > 32 { return false; }
    for i in 0..n { r[i].copy_from_slice(&wrapped[(i + 1) * 8..(i + 2) * 8]); }
    for j in (0..6).rev() {
        for i in (1..=n).rev() {
            // A ^= t ; B = AES-dec(A ‖ R[i]); A = B[0:8]; R[i] = B[8:16]
            let t = (n * j + i) as u64;
            let tb = t.to_be_bytes();
            for k in 0..8 { a[k] ^= tb[k]; }
            let mut blk = [0u8; 16];
            blk[0..8].copy_from_slice(&a);
            blk[8..16].copy_from_slice(&r[i - 1]);
            let d = aes.decrypt_block(&blk);
            a.copy_from_slice(&d[0..8]);
            r[i - 1].copy_from_slice(&d[8..16]);
        }
    }
    for i in 0..n { out[i * 8..i * 8 + 8].copy_from_slice(&r[i]); }
    a == [0xA6; 8]
}

/// Wrap `plain` (len = n*8) with KEK; writes (n+1)*8 bytes to `out`. (The inverse
/// of key_unwrap — handy for round-trip testing the unwrap.)
pub fn key_wrap(kek: &[u8; 16], plain: &[u8], out: &mut [u8]) -> bool {
    if plain.is_empty() || plain.len() % 8 != 0 { return false; }
    let n = plain.len() / 8;
    if n > 32 || out.len() < (n + 1) * 8 { return false; }
    let aes = Aes128::new(kek);
    let mut a = [0xA6u8; 8];
    let mut r = [[0u8; 8]; 32];
    for i in 0..n { r[i].copy_from_slice(&plain[i * 8..i * 8 + 8]); }
    for j in 0..6 {
        for i in 1..=n {
            let mut blk = [0u8; 16];
            blk[0..8].copy_from_slice(&a);
            blk[8..16].copy_from_slice(&r[i - 1]);
            let e = aes.encrypt_block(&blk);
            a.copy_from_slice(&e[0..8]);
            let t = (n * j + i) as u64;
            let tb = t.to_be_bytes();
            for k in 0..8 { a[k] ^= tb[k]; }
            r[i - 1].copy_from_slice(&e[8..16]);
        }
    }
    out[0..8].copy_from_slice(&a);
    for i in 0..n { out[(i + 1) * 8..(i + 2) * 8].copy_from_slice(&r[i]); }
    true
}

// ───────────────────────────── Self-test ────────────────────────────────────

fn eq(a: &[u8], b: &[u8]) -> bool { a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x == y) }

/// Verify against the FIPS-197 (AES-128) and RFC 3394 (128-bit KEK / 128-bit key)
/// known-answer vectors. Cheap enough to run at boot.
pub fn selftest() -> bool {
    // FIPS-197 §C.1 AES-128: key 000102…0f, pt 00112233…ff
    let key = [0u8,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15];
    let pt  = [0x00,0x11,0x22,0x33,0x44,0x55,0x66,0x77,0x88,0x99,0xaa,0xbb,0xcc,0xdd,0xee,0xff];
    let ct  = [0x69,0xc4,0xe0,0xd8,0x6a,0x7b,0x04,0x30,0xd8,0xcd,0xb7,0x80,0x70,0xb4,0xc5,0x5a];
    let aes = Aes128::new(&key);
    if !eq(&aes.encrypt_block(&pt), &ct) { return false; }
    if !eq(&aes.decrypt_block(&ct), &pt) { return false; }
    // RFC 3394 §4.1: 128-bit KEK wraps 128-bit key data.
    let kek = [0u8,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15];
    let kd  = [0x00,0x11,0x22,0x33,0x44,0x55,0x66,0x77,0x88,0x99,0xaa,0xbb,0xcc,0xdd,0xee,0xff];
    let expect = [
        0x1f,0xa6,0x8b,0x0a,0x81,0x12,0xb4,0x47,0xae,0xf3,0x4b,0xd8,0xfb,0x5a,0x7b,0x82,
        0x9d,0x3e,0x86,0x23,0x71,0xd2,0xcf,0xe5,
    ];
    let mut wrapped = [0u8; 24];
    if !key_wrap(&kek, &kd, &mut wrapped) || !eq(&wrapped, &expect) { return false; }
    let mut un = [0u8; 16];
    if !key_unwrap(&kek, &expect, &mut un) || !eq(&un, &kd) { return false; }
    true
}
