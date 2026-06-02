//! WPA2-PSK authentication core (IEEE 802.11i) — the HARDWARE-INDEPENDENT half of
//! bare-metal WiFi. Pure `core`, no alloc, host-testable; verified against canonical
//! published vectors (FIPS 180, RFC 2202, RFC 6070, IEEE 802.11i Annex).
//!
//! This is the key-derivation / authentication layer that sits ABOVE the per-chip
//! radio driver: it turns a passphrase + SSID into the PMK (PBKDF2-HMAC-SHA1) and
//! expands the PMK + the handshake nonces/MACs into the PTK (the 802.11i PRF) that
//! the 4-way handshake installs. None of this needs the radio, so it lands and is
//! verified now; the MMIO/firmware bring-up (brick 2) is the part that needs real
//! Intel hardware QEMU can't emulate. See [[rusty-penguin-net-stack]] for the wired
//! stack and the iwlwifi firmware parser (the sibling host-tested brick).
//!
//! Crypto note: SHA-1 is broken for collision resistance, but WPA2 uses it only
//! inside HMAC/PBKDF2/PRF, where it is still the standard-mandated construction.
//! This is a faithful port of the public reference, not a hardened library.

#![allow(dead_code)]

// ───────────────────────────── SHA-1 (FIPS 180-4) ───────────────────────────

#[derive(Clone)]
pub struct Sha1 {
    h: [u32; 5],
    buf: [u8; 64],
    buf_len: usize,
    total: u64,
}

impl Sha1 {
    pub fn new() -> Self {
        Sha1 {
            h: [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0],
            buf: [0; 64],
            buf_len: 0,
            total: 0,
        }
    }

    fn block(&mut self, blk: &[u8; 64]) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([blk[4 * i], blk[4 * i + 1], blk[4 * i + 2], blk[4 * i + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) =
            (self.h[0], self.h[1], self.h[2], self.h[3], self.h[4]);
        for i in 0..80 {
            let (f, k) = if i < 20 {
                ((b & c) | ((!b) & d), 0x5A82_7999u32)
            } else if i < 40 {
                (b ^ c ^ d, 0x6ED9_EBA1)
            } else if i < 60 {
                ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC)
            } else {
                (b ^ c ^ d, 0xCA62_C1D6)
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        self.h[0] = self.h[0].wrapping_add(a);
        self.h[1] = self.h[1].wrapping_add(b);
        self.h[2] = self.h[2].wrapping_add(c);
        self.h[3] = self.h[3].wrapping_add(d);
        self.h[4] = self.h[4].wrapping_add(e);
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total = self.total.wrapping_add(data.len() as u64);
        if self.buf_len > 0 {
            let need = 64 - self.buf_len;
            let take = need.min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
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

    pub fn finalize(mut self) -> [u8; 20] {
        let bits = self.total.wrapping_mul(8);
        let mut i = self.buf_len;
        self.buf[i] = 0x80;
        i += 1;
        if i > 56 {
            while i < 64 {
                self.buf[i] = 0;
                i += 1;
            }
            let blk = self.buf;
            self.block(&blk);
            i = 0;
        }
        while i < 56 {
            self.buf[i] = 0;
            i += 1;
        }
        self.buf[56..64].copy_from_slice(&bits.to_be_bytes());
        let blk = self.buf;
        self.block(&blk);
        let mut out = [0u8; 20];
        for j in 0..5 {
            out[4 * j..4 * j + 4].copy_from_slice(&self.h[j].to_be_bytes());
        }
        out
    }
}

pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut s = Sha1::new();
    s.update(data);
    s.finalize()
}

// ──────────────────────────── HMAC-SHA1 (RFC 2104) ──────────────────────────

/// Streaming HMAC-SHA1: lets PBKDF2/PRF feed a message in parts without a heap.
pub struct HmacSha1 {
    inner: Sha1,
    opad: [u8; 64],
}

impl HmacSha1 {
    pub fn new(key: &[u8]) -> Self {
        let mut k = [0u8; 64];
        if key.len() > 64 {
            let kh = sha1(key);
            k[..20].copy_from_slice(&kh);
        } else {
            k[..key.len()].copy_from_slice(key);
        }
        let mut ipad = [0x36u8; 64];
        let mut opad = [0x5cu8; 64];
        for i in 0..64 {
            ipad[i] ^= k[i];
            opad[i] ^= k[i];
        }
        let mut inner = Sha1::new();
        inner.update(&ipad);
        HmacSha1 { inner, opad }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    pub fn finalize(self) -> [u8; 20] {
        let ih = self.inner.finalize();
        let mut outer = Sha1::new();
        outer.update(&self.opad);
        outer.update(&ih);
        outer.finalize()
    }
}

pub fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    let mut h = HmacSha1::new(key);
    h.update(msg);
    h.finalize()
}

// ─────────────────────── PBKDF2-HMAC-SHA1 (RFC 2898) ────────────────────────

/// PBKDF2 with HMAC-SHA1 as the PRF. Writes `out.len()` derived bytes.
pub fn pbkdf2_sha1(password: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) {
    let mut block_index: u32 = 1;
    let mut written = 0usize;
    while written < out.len() {
        // U1 = HMAC(P, S || INT_BE(block_index))
        let mut u = {
            let mut h = HmacSha1::new(password);
            h.update(salt);
            h.update(&block_index.to_be_bytes());
            h.finalize()
        };
        let mut t = u;
        // U2..Uc, XOR-accumulated into T.
        for _ in 1..iterations {
            u = hmac_sha1(password, &u);
            for k in 0..20 {
                t[k] ^= u[k];
            }
        }
        let take = (out.len() - written).min(20);
        out[written..written + take].copy_from_slice(&t[..take]);
        written += take;
        block_index += 1;
    }
}

/// WPA/WPA2 passphrase → 256-bit PMK: PBKDF2-HMAC-SHA1(passphrase, SSID, 4096, 32).
/// (IEEE 802.11i §H.4 / RFC 2898). Passphrase is 8..63 printable ASCII; SSID ≤ 32 B.
pub fn wpa_passphrase_to_psk(passphrase: &[u8], ssid: &[u8]) -> [u8; 32] {
    let mut psk = [0u8; 32];
    pbkdf2_sha1(passphrase, ssid, 4096, &mut psk);
    psk
}

// ─────────────────────── IEEE 802.11i PRF + PTK ─────────────────────────────

/// IEEE 802.11i PRF (11.6.1.2): R = HMAC-SHA1(K, A ‖ 0x00 ‖ B ‖ i) for i=0,1,…
/// concatenated and truncated to `out.len()` bytes. `label` is A, `data` is B.
pub fn prf(key: &[u8], label: &[u8], data: &[u8], out: &mut [u8]) {
    let mut counter: u8 = 0;
    let mut written = 0usize;
    while written < out.len() {
        let mut h = HmacSha1::new(key);
        h.update(label);
        h.update(&[0u8]);
        h.update(data);
        h.update(&[counter]);
        let r = h.finalize();
        let take = (out.len() - written).min(20);
        out[written..written + take].copy_from_slice(&r[..take]);
        written += take;
        counter += 1;
    }
}

/// Order two equal-length byte strings and write min‖max into `out` (the 802.11i
/// canonicalisation used for the PTK so both peers derive the same key).
fn min_max(a: &[u8], b: &[u8], out: &mut [u8]) {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    out[..lo.len()].copy_from_slice(lo);
    out[lo.len()..lo.len() + hi.len()].copy_from_slice(hi);
}

/// Pairwise Transient Key expansion (4-way handshake): PTK = PRF(PMK,
/// "Pairwise key expansion", min(AA,SPA)‖max(AA,SPA)‖min(ANonce,SNonce)‖max(…)).
/// `aa`/`spa` are the authenticator/supplicant MACs (6 B); nonces are 32 B.
/// Writes `out.len()` bytes (48 = CCMP, 64 = TKIP).
pub fn ptk(pmk: &[u8], aa: &[u8; 6], spa: &[u8; 6], anonce: &[u8; 32], snonce: &[u8; 32], out: &mut [u8]) {
    let mut data = [0u8; 12 + 64];
    min_max(aa, spa, &mut data[0..12]);
    min_max(anonce, snonce, &mut data[12..76]);
    prf(pmk, b"Pairwise key expansion", &data[..76], out);
}

// ───────────────────────────── Self-test ────────────────────────────────────

fn eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x == y)
}

/// Verify the primitives against canonical published vectors. Returns true iff all
/// pass. Cheap enough to run at boot; the host test (tools/wpa2_test.rs) prints each.
pub fn selftest() -> bool {
    // FIPS 180-1: SHA1("abc")
    if !eq(&sha1(b"abc"),
           &[0xa9,0x99,0x3e,0x36,0x47,0x06,0x81,0x6a,0xba,0x3e,
             0x25,0x71,0x78,0x50,0xc2,0x6c,0x9c,0xd0,0xd8,0x9d]) { return false; }
    // SHA1("")
    if !eq(&sha1(b""),
           &[0xda,0x39,0xa3,0xee,0x5e,0x6b,0x4b,0x0d,0x32,0x55,
             0xbf,0xef,0x95,0x60,0x18,0x90,0xaf,0xd8,0x07,0x09]) { return false; }
    // RFC 2202 HMAC-SHA1 case 1: key=0x0b×20, data="Hi There"
    if !eq(&hmac_sha1(&[0x0b; 20], b"Hi There"),
           &[0xb6,0x17,0x31,0x86,0x55,0x05,0x72,0x64,0xe2,0x8b,
             0xc0,0xb6,0xfb,0x37,0x8c,0x8e,0xf1,0x46,0xbe,0x00]) { return false; }
    // RFC 2202 HMAC-SHA1 case 2: key="Jefe", data="what do ya want for nothing?"
    if !eq(&hmac_sha1(b"Jefe", b"what do ya want for nothing?"),
           &[0xef,0xfc,0xdf,0x6a,0xe5,0xeb,0x2f,0xa2,0xd2,0x74,
             0x16,0xd5,0xf1,0x84,0xdf,0x9c,0x25,0x9a,0x7c,0x79]) { return false; }
    // RFC 6070 PBKDF2-HMAC-SHA1: P="password", S="salt", c=1, dkLen=20
    let mut d = [0u8; 20];
    pbkdf2_sha1(b"password", b"salt", 1, &mut d);
    if !eq(&d, &[0x0c,0x60,0xc8,0x0f,0x96,0x1f,0x0e,0x71,0xf3,0xa9,
                 0xb5,0x24,0xaf,0x60,0x12,0x06,0x2f,0xe0,0x37,0xa6]) { return false; }
    // RFC 6070: c=2
    pbkdf2_sha1(b"password", b"salt", 2, &mut d);
    if !eq(&d, &[0xea,0x6c,0x01,0x4d,0xc7,0x2d,0x6f,0x8c,0xcd,0x1e,
                 0xd9,0x2a,0xce,0x1d,0x41,0xf0,0xd8,0xde,0x89,0x57]) { return false; }
    // RFC 6070: c=4096
    pbkdf2_sha1(b"password", b"salt", 4096, &mut d);
    if !eq(&d, &[0x4b,0x00,0x79,0x01,0xb7,0x65,0x48,0x9a,0xbe,0xad,
                 0x49,0xd9,0x26,0xf7,0x21,0xd0,0x65,0xa4,0x29,0xc1]) { return false; }
    // IEEE 802.11i §H.4 WPA PSK: passphrase="password", SSID="IEEE"
    let psk = wpa_passphrase_to_psk(b"password", b"IEEE");
    if !eq(&psk, &[0xf4,0x2c,0x6f,0xc5,0x2d,0xf0,0xeb,0xef,0x9e,0xbb,0x4b,0x90,0xb3,0x8a,0x5f,0x90,
                   0x2e,0x83,0xfe,0x1b,0x13,0x5a,0x70,0xe2,0x3a,0xed,0x76,0x2e,0x97,0x10,0xa1,0x2e]) { return false; }
    // PTK structure/determinism: same inputs reproduce; a different SNonce diverges;
    // the 48-byte CCMP key splits into KCK(16)‖KEK(16)‖TK(16).
    let aa = [0x00,0x01,0x02,0x03,0x04,0x05];
    let spa = [0x10,0x11,0x12,0x13,0x14,0x15];
    let anonce = [0x22u8; 32];
    let snonce1 = [0x33u8; 32];
    let snonce2 = [0x44u8; 32];
    let mut k1 = [0u8; 48];
    let mut k1b = [0u8; 48];
    let mut k2 = [0u8; 48];
    ptk(&psk, &aa, &spa, &anonce, &snonce1, &mut k1);
    ptk(&psk, &aa, &spa, &anonce, &snonce1, &mut k1b);
    ptk(&psk, &aa, &spa, &anonce, &snonce2, &mut k2);
    if !eq(&k1, &k1b) { return false; }     // deterministic
    if eq(&k1, &k2) { return false; }        // nonce actually mixed in
    true
}
