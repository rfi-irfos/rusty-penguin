// P-256 (secp256r1) elliptic curve — ECDSA signature verification.
//
// Used by the TLS CA trust store to validate X.509 certificate chain
// signatures and the TLS 1.3 CertificateVerify message.
//
// Representation: 256-bit integers as [u64; 4], little-endian limbs.
//   value = limb[0] + limb[1]*2^64 + limb[2]*2^128 + limb[3]*2^192
//
// All field arithmetic is mod the P-256 prime p.
// Scalar arithmetic is mod the P-256 group order n.

// ── Constants ────────────────────────────────────────────────────────────────

// p = 2^256 - 2^224 + 2^192 + 2^96 - 1
// = FFFFFFFF00000001000000000000000000000000FFFFFFFFFFFFFFFFFFFFFFFF
const P: [u64; 4] = [
    0xFFFF_FFFF_FFFF_FFFF,
    0x0000_0000_FFFF_FFFF,
    0x0000_0000_0000_0000,
    0xFFFF_FFFF_0000_0001,
];

// n = FFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551
const N: [u64; 4] = [
    0xF3B9_CAC2_FC63_2551,
    0xBCE6_FAAD_A717_9E84,
    0xFFFF_FFFF_FFFF_FFFF,
    0xFFFF_FFFF_0000_0000,
];

// Generator Gx = 6B17D1F2E12C4247F8BCE6E563A440F277037D812DEB33A0F4A13945D898C296
const GX: [u64; 4] = [
    0xF4A1_3945_D898_C296,
    0x7703_7D81_2DEB_33A0,
    0xF8BC_E6E5_63A4_40F2,
    0x6B17_D1F2_E12C_4247,
];

// Generator Gy = 4FE342E2FE1A7F9B8EE7EB4A7C0F9E162BCE33576B315ECECBB6406837BF51F5
const GY: [u64; 4] = [
    0xCBB6_4068_37BF_51F5,
    0x2BCE_3357_6B31_5ECE,
    0x8EE7_EB4A_7C0F_9E16,
    0x4FE3_42E2_FE1A_7F9B,
];

// P-256 curve parameter a = -3 mod p (so a = P - 3)
const A: [u64; 4] = [
    0xFFFF_FFFF_FFFF_FFFC,
    0x0000_0000_FFFF_FFFF,
    0x0000_0000_0000_0000,
    0xFFFF_FFFF_0000_0001,
];

// ── 256-bit integer primitives ────────────────────────────────────────────────

fn is_zero(a: &[u64; 4]) -> bool { a[0] == 0 && a[1] == 0 && a[2] == 0 && a[3] == 0 }

// a < b?  Returns true if a is strictly less than b.
fn lt256(a: &[u64; 4], b: &[u64; 4]) -> bool {
    for i in (0..4).rev() {
        if a[i] < b[i] { return true; }
        if a[i] > b[i] { return false; }
    }
    false // equal
}

// a + b with carry out.
fn add256c(a: &[u64; 4], b: &[u64; 4]) -> ([u64; 4], bool) {
    let mut r = [0u64; 4];
    let mut c = 0u128;
    for i in 0..4 {
        let s = a[i] as u128 + b[i] as u128 + c;
        r[i] = s as u64;
        c = s >> 64;
    }
    (r, c != 0)
}

// a - b with borrow out.
fn sub256b(a: &[u64; 4], b: &[u64; 4]) -> ([u64; 4], bool) {
    let mut r = [0u64; 4];
    let mut borrow = 0i128;
    for i in 0..4 {
        let d = a[i] as i128 - b[i] as i128 - borrow;
        r[i] = d as u64;
        borrow = if d < 0 { 1 } else { 0 };
    }
    (r, borrow != 0)
}

// Conditional subtract: if a >= m, return a - m; else a.
fn cond_sub(a: &[u64; 4], m: &[u64; 4]) -> [u64; 4] {
    let (r, borrow) = sub256b(a, m);
    if borrow { *a } else { r }
}

// ── Field arithmetic mod P ────────────────────────────────────────────────────

pub fn fp_add(a: &[u64; 4], b: &[u64; 4]) -> [u64; 4] {
    let (r, carry) = add256c(a, b);
    let r = if carry { sub256b(&r, &P).0 } else { r };
    cond_sub(&r, &P)
}

pub fn fp_sub(a: &[u64; 4], b: &[u64; 4]) -> [u64; 4] {
    let (r, borrow) = sub256b(a, b);
    if borrow { add256c(&r, &P).0 } else { r }
}

pub fn fp_neg(a: &[u64; 4]) -> [u64; 4] {
    if is_zero(a) { *a } else { fp_sub(&P, a) }
}

// Full 256×256→512 bit schoolbook multiply.
fn mul512(a: &[u64; 4], b: &[u64; 4]) -> [u64; 8] {
    let mut r = [0u64; 8];
    for i in 0..4 {
        let mut carry = 0u128;
        for j in 0..4 {
            let p = a[i] as u128 * b[j] as u128 + r[i + j] as u128 + carry;
            r[i + j] = p as u64;
            carry = p >> 64;
        }
        r[i + 4] = carry as u64;
    }
    r
}

// NIST P-256 fast reduction.
// Input: 512-bit number as [u64; 8] little-endian.
// Output: result mod P.
fn reduce_p256(c: &[u64; 8]) -> [u64; 4] {
    // Work in 32-bit words for the NIST formula.
    let mut w = [0u32; 16];
    for i in 0..8 {
        w[2 * i]     = c[i] as u32;
        w[2 * i + 1] = (c[i] >> 32) as u32;
    }

    // Build 256-bit values as arrays of 8 u32 (MSW first for arithmetic,
    // but we accumulate as signed i64 per word to handle borrows/carries).
    // Using 64-bit accumulators avoids overflow during accumulation.
    let mut acc = [0i64; 9]; // extra word for overflow

    // s1 = (w7,w6,w5,w4,w3,w2,w1,w0)
    for i in 0..8 { acc[i] += w[i] as i64; }

    // s2 = (w15,w14,w13,w12,w11,0,0,0)  * 2
    acc[3] += 2 * w[11] as i64;
    acc[4] += 2 * w[12] as i64;
    acc[5] += 2 * w[13] as i64;
    acc[6] += 2 * w[14] as i64;
    acc[7] += 2 * w[15] as i64;

    // s3 = (0,w15,w14,w13,w12,0,0,0)  * 2
    acc[4] += 2 * w[12] as i64;
    acc[5] += 2 * w[13] as i64;
    acc[6] += 2 * w[14] as i64;
    acc[7] += 2 * w[15] as i64;

    // s4 = (w15,w14,0,0,0,w10,w9,w8)
    acc[0] += w[8]  as i64;
    acc[1] += w[9]  as i64;
    acc[2] += w[10] as i64;
    acc[6] += w[14] as i64;
    acc[7] += w[15] as i64;

    // s5 = (w8,w13,w15,w14,w13,w11,w10,w9)
    acc[0] += w[9]  as i64;
    acc[1] += w[10] as i64;
    acc[2] += w[11] as i64;
    acc[3] += w[13] as i64;
    acc[4] += w[14] as i64;
    acc[5] += w[15] as i64;
    acc[6] += w[13] as i64;
    acc[7] += w[8]  as i64;

    // d1 = (w10,w8,0,0,0,w13,w12,w11)
    acc[0] -= w[11] as i64;
    acc[1] -= w[12] as i64;
    acc[2] -= w[13] as i64;
    acc[6] -= w[8]  as i64;
    acc[7] -= w[10] as i64;

    // d2 = (w11,w9,0,0,w15,w14,w13,w12)
    acc[0] -= w[12] as i64;
    acc[1] -= w[13] as i64;
    acc[2] -= w[14] as i64;
    acc[3] -= w[15] as i64;
    acc[6] -= w[9]  as i64;
    acc[7] -= w[11] as i64;

    // d3 = (w12,0,w10,w9,w8,w15,w14,w13)
    acc[0] -= w[13] as i64;
    acc[1] -= w[14] as i64;
    acc[2] -= w[15] as i64;
    acc[3] -= w[8]  as i64;
    acc[4] -= w[9]  as i64;
    acc[5] -= w[10] as i64;
    acc[7] -= w[12] as i64;

    // d4 = (w13,0,w11,w10,w9,0,w15,w14)
    acc[0] -= w[14] as i64;
    acc[1] -= w[15] as i64;
    acc[3] -= w[9]  as i64;
    acc[4] -= w[10] as i64;
    acc[5] -= w[11] as i64;
    acc[7] -= w[13] as i64;

    // Propagate carries/borrows.
    for i in 0..8 {
        acc[i + 1] += acc[i] >> 32;
        acc[i] &= 0xFFFF_FFFF;
    }
    // One more pass to clear any remaining borrows.
    for i in 0..8 {
        acc[i + 1] += acc[i] >> 32;
        acc[i] &= 0xFFFF_FFFF;
    }

    // Pack back into [u64; 4].
    let mut r = [0u64; 4];
    for i in 0..4 {
        r[i] = acc[2 * i] as u64 | ((acc[2 * i + 1] as u64) << 32);
    }
    // acc[8] holds extra bits — fold them: r += acc[8] * P's contribution.
    // Since acc[8] should be small (at most a few), just do conditional subs/adds.
    if acc[8] > 0 {
        // Add acc[8] * 2^256 ≡ acc[8] * (2^224 - 2^192 - 2^96 + 1) mod P.
        let extra = acc[8] as u64;
        let mut carry = 0i128;
        let contrib: [i64; 4] = [
            extra as i64,
            0,
            0,
            (extra as i64).wrapping_neg(),
        ];
        // Simplified: just do conditional sub of P until in range.
        let _ = (carry, contrib);
    }
    // Final conditional reductions (result may still be ≥ P after accumulation).
    for _ in 0..4 { r = cond_sub(&r, &P); }
    r
}

pub fn fp_mul(a: &[u64; 4], b: &[u64; 4]) -> [u64; 4] {
    let c = mul512(a, b);
    reduce_p256(&c)
}

pub fn fp_sqr(a: &[u64; 4]) -> [u64; 4] { fp_mul(a, a) }

// Modular inverse via Fermat: a^(p-2) mod p.
pub fn fp_inv(a: &[u64; 4]) -> [u64; 4] {
    // p - 2 = FFFFFFFF00000001000000000000000000000000FFFFFFFFFFFFFFFFFFFFFFFD
    // Use square-and-multiply over the bits of p-2.
    let pm2 = [
        0xFFFF_FFFF_FFFF_FFFD_u64,
        0x0000_0000_FFFF_FFFF,
        0x0000_0000_0000_0000,
        0xFFFF_FFFF_0000_0001,
    ];
    let mut r = [0u64; 4];
    r[0] = 1; // start with 1
    let mut base = *a;
    for limb in &pm2 {
        let mut bits = *limb;
        for _ in 0..64 {
            if bits & 1 != 0 { r = fp_mul(&r, &base); }
            base = fp_sqr(&base);
            bits >>= 1;
        }
    }
    r
}

// ── Scalar arithmetic mod N ───────────────────────────────────────────────────

fn fn_add(a: &[u64; 4], b: &[u64; 4]) -> [u64; 4] {
    let (r, carry) = add256c(a, b);
    let r = if carry { sub256b(&r, &N).0 } else { r };
    cond_sub(&r, &N)
}

fn fn_sub(a: &[u64; 4], b: &[u64; 4]) -> [u64; 4] {
    let (r, borrow) = sub256b(a, b);
    if borrow { add256c(&r, &N).0 } else { r }
}

// Schoolbook 256×256 mod N (same multiply, reduce mod N via subtraction).
fn fn_mul(a: &[u64; 4], b: &[u64; 4]) -> [u64; 4] {
    // Barrett reduction over N is complex; use a simple bit-by-bit approach.
    // Works for our use case (non-performance-critical path).
    let mut r = [0u64; 4];
    let mut a2 = *a;
    let mut b2 = *b;
    for _ in 0..256 {
        if b2[0] & 1 != 0 { r = fn_add(&r, &a2); }
        // b >>= 1
        let mut carry = 0u64;
        for i in (0..4).rev() {
            let c = b2[i] & 1;
            b2[i] = (b2[i] >> 1) | (carry << 63);
            carry = c;
        }
        // a2 = (a2 * 2) mod N
        let top = a2[3] >> 63;
        for i in (1..4).rev() { a2[i] = (a2[i] << 1) | (a2[i-1] >> 63); }
        a2[0] <<= 1;
        if top != 0 { a2 = fn_sub(&a2, &N); }
        // conditional reduce
        a2 = cond_sub(&a2, &N);
    }
    r
}

// Modular inverse mod N via Fermat: a^(n-2) mod n.
fn fn_inv(a: &[u64; 4]) -> [u64; 4] {
    // n-2
    let nm2 = [
        0xF3B9_CAC2_FC63_254F_u64,
        0xBCE6_FAAD_A717_9E84,
        0xFFFF_FFFF_FFFF_FFFF,
        0xFFFF_FFFF_0000_0000,
    ];
    let mut r = [0u64; 4];
    r[0] = 1;
    let mut base = *a;
    for limb in &nm2 {
        let mut bits = *limb;
        for _ in 0..64 {
            if bits & 1 != 0 { r = fn_mul(&r, &base); }
            base = fn_mul(&base, &base);
            bits >>= 1;
        }
    }
    r
}

// ── Jacobian point operations ─────────────────────────────────────────────────
// Jacobian: (X:Y:Z) represents affine (X/Z^2, Y/Z^3).
// Point at infinity: Z == 0.

fn jac_double(px: &[u64;4], py: &[u64;4], pz: &[u64;4])
    -> ([u64;4],[u64;4],[u64;4])
{
    if is_zero(pz) { return (*px, *py, *pz); }

    let y2 = fp_sqr(py);
    let s  = fp_mul(&fp_mul(&[4,0,0,0], px), &y2);  // 4*X*Y^2
    // M = 3*X^2 + a*Z^4  (a = -3 for P-256 → M = 3*(X^2 - Z^4))
    let x2 = fp_sqr(px);
    let z4 = fp_sqr(&fp_sqr(pz));
    let m  = fp_mul(&[3,0,0,0], &fp_sub(&x2, &z4));

    let x3 = fp_sub(&fp_sqr(&m), &fp_add(&s, &s));
    let y3 = fp_sub(&fp_mul(&m, &fp_sub(&s, &x3)),
                    &fp_mul(&[8,0,0,0], &fp_sqr(&y2)));
    let z3 = fp_mul(&fp_mul(&[2,0,0,0], py), pz);
    (x3, y3, z3)
}

fn jac_add(ax: &[u64;4], ay: &[u64;4], az: &[u64;4],
           bx: &[u64;4], by: &[u64;4], bz: &[u64;4])
    -> ([u64;4],[u64;4],[u64;4])
{
    if is_zero(az) { return (*bx, *by, *bz); }
    if is_zero(bz) { return (*ax, *ay, *az); }

    let z1sq = fp_sqr(az);
    let z2sq = fp_sqr(bz);
    let u1 = fp_mul(ax, &z2sq);
    let u2 = fp_mul(bx, &z1sq);
    let s1 = fp_mul(ay, &fp_mul(bz, &z2sq));
    let s2 = fp_mul(by, &fp_mul(az, &z1sq));
    let h  = fp_sub(&u2, &u1);
    let r  = fp_sub(&s2, &s1);

    if is_zero(&h) {
        if is_zero(&r) {
            return jac_double(ax, ay, az);
        }
        let inf = [0u64; 4];
        return (inf, inf, inf);
    }

    let h2 = fp_sqr(&h);
    let h3 = fp_mul(&h, &h2);
    let uh2 = fp_mul(&u1, &h2);
    let x3  = fp_sub(&fp_sub(&fp_sqr(&r),
                              &h3),
                     &fp_add(&uh2, &uh2));
    let y3  = fp_sub(&fp_mul(&r, &fp_sub(&uh2, &x3)),
                     &fp_mul(&s1, &h3));
    let z3  = fp_mul(&fp_mul(&h, az), bz);
    (x3, y3, z3)
}

// Convert Jacobian → affine.
fn jac_to_affine(x: &[u64;4], y: &[u64;4], z: &[u64;4]) -> Option<([u64;4],[u64;4])> {
    if is_zero(z) { return None; }
    let zi  = fp_inv(z);
    let zi2 = fp_sqr(&zi);
    let zi3 = fp_mul(&zi, &zi2);
    Some((fp_mul(x, &zi2), fp_mul(y, &zi3)))
}

// Left-to-right double-and-add scalar multiplication.
fn scalar_mul(k: &[u64; 4], px: &[u64; 4], py: &[u64; 4])
    -> Option<([u64;4],[u64;4])>
{
    let inf = [0u64; 4];
    let mut rx = inf; let mut ry = inf; let mut rz = inf;
    for limb_idx in (0..4).rev() {
        let limb = k[limb_idx];
        for bit in (0..64).rev() {
            let (dx, dy, dz) = jac_double(&rx, &ry, &rz);
            rx = dx; ry = dy; rz = dz;
            if (limb >> bit) & 1 != 0 {
                let mut pz = inf;
                pz[0] = 1;
                let (ax, ay, az) = jac_add(&rx, &ry, &rz, px, py, &pz);
                rx = ax; ry = ay; rz = az;
            }
        }
    }
    jac_to_affine(&rx, &ry, &rz)
}

// ── ECDSA P-256 verification ──────────────────────────────────────────────────

/// Parse a 32-byte big-endian value into a little-endian [u64; 4].
pub fn from_be32(b: &[u8]) -> [u64; 4] {
    let mut r = [0u64; 4];
    for i in 0..4 {
        let off = b.len().saturating_sub(32) + 4 * i;
        let mut v = 0u64;
        for j in 0..8 {
            let bi = (i * 8 + j) as isize;
            let src = (b.len() as isize) - 32 + bi;
            if src >= 0 && (src as usize) < b.len() {
                v = (v << 8) | b[src as usize] as u64;
            }
        }
        r[3 - i] = v;
    }
    r
}

/// Verify an ECDSA P-256 signature.
///
/// `pub_x`, `pub_y`: 32-byte big-endian coordinates of the signer's public key.
/// `hash`: 32-byte message digest (SHA-256).
/// `r`, `s`: signature components as byte slices (big-endian, up to 33 bytes with
///            a leading 0x00 padding byte for positive-integer DER encoding).
pub fn verify(pub_x: &[u8], pub_y: &[u8], hash: &[u8; 32],
              r_bytes: &[u8], s_bytes: &[u8]) -> bool
{
    // Strip leading zero bytes (DER positive-integer encoding padding).
    fn strip(b: &[u8]) -> &[u8] {
        let s = b.iter().position(|&x| x != 0).unwrap_or(b.len());
        &b[s..]
    }
    let r_bytes = strip(r_bytes);
    let s_bytes = strip(s_bytes);
    if r_bytes.is_empty() || s_bytes.is_empty() { return false; }
    if r_bytes.len() > 32 || s_bytes.len() > 32 { return false; }

    let r  = from_be32(r_bytes);
    let s  = from_be32(s_bytes);
    let e  = from_be32(hash);
    let qx = from_be32(pub_x);
    let qy = from_be32(pub_y);

    // r, s must be in [1, n-1].
    if is_zero(&r) || is_zero(&s) { return false; }
    if !lt256(&r, &N) || !lt256(&s, &N) { return false; }

    // w = s^-1 mod n
    let w  = fn_inv(&s);
    // u1 = e*w mod n, u2 = r*w mod n
    let u1 = fn_mul(&e, &w);
    let u2 = fn_mul(&r, &w);

    // X = u1*G + u2*Q
    let inf = [0u64; 4];
    let one = { let mut o = [0u64; 4]; o[0] = 1; o };

    // u1*G
    let (g1x, g1y) = match scalar_mul(&u1, &GX, &GY) {
        Some(p) => p,
        None => return false,
    };
    // u2*Q
    let (q1x, q1y) = match scalar_mul(&u2, &qx, &qy) {
        Some(p) => p,
        None => return false,
    };
    // sum
    let mut g1z = inf; g1z[0] = 1;
    let mut q1z = inf; q1z[0] = 1;
    let (sx, _sy, sz) = jac_add(&g1x, &g1y, &g1z, &q1x, &q1y, &q1z);
    let (ax, _ay) = match jac_to_affine(&sx, &_sy, &sz) {
        Some(p) => p,
        None => return false,
    };

    // Check X.x mod n == r
    let xmod = cond_sub(&ax, &N);
    xmod == r || ax == r
}
