//! Minimal big-integer modular exponentiation for RSA signature verification.
//! Just enough to compute `sig^e mod n` for RSA-PKCS#1 v1.5 cert signatures —
//! the half of a CA trust store that P-256 (`p256.rs`) doesn't cover. Numbers
//! are little-endian `u64` limbs; the modulus is odd (RSA), so we use Montgomery
//! (CIOS) multiplication. Verification only, so constant-time is not a concern.
//!
//! The CIOS algorithm and the surrounding mod_exp are host-fuzzed against real
//! OpenSSL RSA signatures (tools/rsa_test.rs).

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;

/// -n^{-1} mod 2^64, used by the Montgomery reduction step.
fn n0inv(n0: u64) -> u64 {
    // Newton iteration for the inverse of an odd number mod 2^64.
    let mut x = 1u64;
    for _ in 0..6 {
        // x = x * (2 - n0 * x)  (mod 2^64)
        x = x.wrapping_mul(2u64.wrapping_sub(n0.wrapping_mul(x)));
    }
    x.wrapping_neg()
}

/// Big-endian bytes → little-endian u64 limbs, padded/truncated to `k` limbs.
fn from_be(bytes: &[u8], k: usize) -> Vec<u64> {
    let mut limbs = vec![0u64; k];
    // Walk the input from the least-significant byte (end) up.
    let mut bit = 0usize;
    for &b in bytes.iter().rev() {
        let limb = bit / 64;
        let shift = (bit % 64) as u32;
        if limb < k {
            limbs[limb] |= (b as u64) << shift;
            if shift > 56 && limb + 1 < k {
                limbs[limb + 1] |= (b as u64) >> (64 - shift);
            }
        }
        bit += 8;
    }
    limbs
}

/// Little-endian limbs → big-endian bytes, length `k*8` (leading zeros kept).
fn to_be(limbs: &[u64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(limbs.len() * 8);
    for &l in limbs.iter().rev() {
        out.extend_from_slice(&l.to_be_bytes());
    }
    out
}

/// a >= b for equal-length little-endian limbs.
fn ge(a: &[u64], b: &[u64]) -> bool {
    for i in (0..a.len()).rev() {
        if a[i] != b[i] {
            return a[i] > b[i];
        }
    }
    true
}

/// a -= b (a >= b assumed), equal length.
fn sub_assign(a: &mut [u64], b: &[u64]) {
    let mut borrow = 0u128;
    for i in 0..a.len() {
        let v = (a[i] as u128).wrapping_sub(b[i] as u128).wrapping_sub(borrow);
        a[i] = v as u64;
        borrow = (v >> 64) & 1;
    }
}

/// Montgomery product: returns a*b*R^{-1} mod n, where R = 2^(64*k). CIOS form.
fn mont_mul(a: &[u64], b: &[u64], n: &[u64], n0i: u64) -> Vec<u64> {
    let s = n.len();
    let mut t = vec![0u64; s + 2];
    for i in 0..s {
        // t += a * b[i]
        let bi = b[i] as u128;
        let mut c: u128 = 0;
        for j in 0..s {
            let x = t[j] as u128 + (a[j] as u128) * bi + c;
            t[j] = x as u64;
            c = x >> 64;
        }
        let x = t[s] as u128 + c;
        t[s] = x as u64;
        t[s + 1] = (x >> 64) as u64;

        // m = t[0] * n0inv mod 2^64; t = (t + m*n) / 2^64
        let m = (t[0] as u128 * n0i as u128) as u64 as u128;
        let x = t[0] as u128 + m * (n[0] as u128);
        let mut c2 = x >> 64;
        for j in 1..s {
            let x = t[j] as u128 + m * (n[j] as u128) + c2;
            t[j - 1] = x as u64;
            c2 = x >> 64;
        }
        let x = t[s] as u128 + c2;
        t[s - 1] = x as u64;
        t[s] = t[s + 1] + (x >> 64) as u64;
    }
    let mut res = t[..s].to_vec();
    // One conditional subtract: result may be in [0, 2n).
    if t[s] != 0 || ge(&res, n) {
        sub_assign(&mut res, n);
    }
    res
}

/// Compute `base^exp mod n`, all big-endian byte strings. `n` must be odd.
/// Returns big-endian bytes of length `n.len()` rounded up to whole limbs.
pub fn mod_exp(base: &[u8], exp: &[u8], n: &[u8]) -> Vec<u8> {
    // Trim leading zero bytes of n to size the modulus, then limb-count.
    let n_trim = {
        let mut i = 0;
        while i + 1 < n.len() && n[i] == 0 {
            i += 1;
        }
        &n[i..]
    };
    let k = (n_trim.len() + 7) / 8;
    if k == 0 {
        return Vec::new();
    }
    let nl = from_be(n_trim, k);
    if nl[0] & 1 == 0 {
        return Vec::new(); // even modulus unsupported (never happens for RSA)
    }
    let n0i = n0inv(nl[0]);

    // RR = R^2 mod n, R = 2^(64k). R^2 = 2^(128k), so double 1 exactly 128k times.
    let mut rr = vec![0u64; k];
    rr[0] = 1;
    for _ in 0..(128 * k) {
        // rr <<= 1
        let mut carry = 0u64;
        for limb in rr.iter_mut() {
            let nc = *limb >> 63;
            *limb = (*limb << 1) | carry;
            carry = nc;
        }
        if carry != 0 || ge(&rr, &nl) {
            sub_assign(&mut rr, &nl);
        }
    }

    let mut base_l = from_be(base, k);
    if ge(&base_l, &nl) {
        sub_assign(&mut base_l, &nl); // sig < n for RSA; cheap safety
    }
    // a_mont = base * R mod n  (= mont_mul(base, RR))
    let a_mont = mont_mul(&base_l, &rr, &nl, n0i);
    // result = 1 in Montgomery domain = R mod n = mont_mul(1, RR)
    let mut one = vec![0u64; k];
    one[0] = 1;
    let mut result = mont_mul(&one, &rr, &nl, n0i);

    // Square-and-multiply over the exponent bits, MSB first.
    let mut started = false;
    for &byte in exp {
        for bit in (0..8).rev() {
            if started {
                result = mont_mul(&result, &result, &nl, n0i);
            }
            if (byte >> bit) & 1 == 1 {
                if !started {
                    result = a_mont.clone();
                    started = true;
                } else {
                    result = mont_mul(&result, &a_mont, &nl, n0i);
                }
            }
        }
    }
    if !started {
        // exponent was zero → result is 1
        result = one;
    } else {
        // convert out of Montgomery domain: mont_mul(result, 1)
        result = mont_mul(&result, &one, &nl, n0i);
    }
    to_be(&result)
}
