// Balanced ternary arithmetic for Rusty Penguin.
// Operates on Tryte (9 trits, range -9841..+9841).
#![no_std]
use ternary_core::{Trit, Tryte};

/// Multiply two Trytes. Returns (low_word, high_word) — the full double-width product.
pub fn mul_tryte(a: Tryte, b: Tryte) -> (Tryte, Tryte) {
    let product = a.to_i32() as i64 * b.to_i32() as i64;
    let lo = ((product % 19683) + 9841).rem_euclid(19683) - 9841;
    let hi = (product - lo) / 19683;
    (Tryte::from_i32(lo as i32), Tryte::from_i32(hi as i32))
}

/// Divide two Trytes. Returns (quotient, remainder).
/// Division by zero returns (ZERO, dividend).
pub fn div_tryte(a: Tryte, b: Tryte) -> (Tryte, Tryte) {
    let bv = b.to_i32();
    if bv == 0 {
        return (Tryte::ZERO, a);
    }
    let av = a.to_i32();
    (Tryte::from_i32(av / bv), Tryte::from_i32(av % bv))
}

/// Ternary modulo. Sign follows the dividend (like Rust's %).
pub fn mod_tryte(a: Tryte, b: Tryte) -> Tryte {
    div_tryte(a, b).1
}

/// Absolute value of a Tryte.
pub fn abs_tryte(a: Tryte) -> Tryte {
    if a.to_i32() < 0 { -a } else { a }
}

/// Ternary consensus: Pos∧Pos=Pos, Neg∧Neg=Neg, else Zero.
/// The balanced ternary analogue of AND-gating.
pub fn consensus(a: Trit, b: Trit) -> Trit {
    match (a, b) {
        (Trit::Pos, Trit::Pos) => Trit::Pos,
        (Trit::Neg, Trit::Neg) => Trit::Neg,
        _ => Trit::Zero,
    }
}

/// Ternary any: returns the non-Zero trit when only one is non-Zero, else Zero.
/// Analogous to XOR in binary.
pub fn any(a: Trit, b: Trit) -> Trit {
    match (a, b) {
        (Trit::Zero, x) | (x, Trit::Zero) => x,
        _ => Trit::Zero,
    }
}

/// Scale a Tryte by a Trit: Pos=identity, Zero=zero out, Neg=negate.
/// Fundamental one-trit conditional transform.
pub fn scale(a: Tryte, s: Trit) -> Tryte {
    match s {
        Trit::Pos  => a,
        Trit::Zero => Tryte::ZERO,
        Trit::Neg  => -a,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ternary_core::Tryte;

    #[test]
    fn test_mul_basic() {
        let (lo, hi) = mul_tryte(Tryte::from_i32(6), Tryte::from_i32(7));
        assert_eq!(hi.to_i32() * 19683 + lo.to_i32(), 42);
    }

    #[test]
    fn test_mul_negative() {
        let (lo, hi) = mul_tryte(Tryte::from_i32(-5), Tryte::from_i32(9));
        assert_eq!(hi.to_i32() * 19683 + lo.to_i32(), -45);
    }

    #[test]
    fn test_div_basic() {
        let (q, r) = div_tryte(Tryte::from_i32(42), Tryte::from_i32(6));
        assert_eq!(q.to_i32(), 7);
        assert_eq!(r.to_i32(), 0);
    }

    #[test]
    fn test_div_with_remainder() {
        let (q, r) = div_tryte(Tryte::from_i32(17), Tryte::from_i32(5));
        assert_eq!(q.to_i32(), 3);
        assert_eq!(r.to_i32(), 2);
    }

    #[test]
    fn test_div_by_zero() {
        let (q, r) = div_tryte(Tryte::from_i32(10), Tryte::ZERO);
        assert_eq!(q.to_i32(), 0);
        assert_eq!(r.to_i32(), 10);
    }

    #[test]
    fn test_consensus() {
        assert_eq!(consensus(Trit::Pos, Trit::Pos), Trit::Pos);
        assert_eq!(consensus(Trit::Neg, Trit::Neg), Trit::Neg);
        assert_eq!(consensus(Trit::Pos, Trit::Neg), Trit::Zero);
    }

    #[test]
    fn test_scale() {
        let a = Tryte::from_i32(42);
        assert_eq!(scale(a, Trit::Pos).to_i32(),   42);
        assert_eq!(scale(a, Trit::Zero).to_i32(),   0);
        assert_eq!(scale(a, Trit::Neg).to_i32(),  -42);
    }
}
