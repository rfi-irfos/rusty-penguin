use std::ops::{Add, Neg};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(i8)]
pub enum Trit {
    Neg = -1,
    Zero = 0,
    Pos = 1,
}

impl Trit {
    pub fn from_i8(val: i8) -> Option<Self> {
        match val {
            -1 => Some(Trit::Neg),
            0 => Some(Trit::Zero),
            1 => Some(Trit::Pos),
            _ => None,
        }
    }

    pub fn to_i8(self) -> i8 {
        self as i8
    }

    /// Full adder for trits. Returns (Sum, Carry).
    pub fn full_add(self, other: Trit, carry_in: Trit) -> (Trit, Trit) {
        let sum_val = self.to_i8() + other.to_i8() + carry_in.to_i8();
        match sum_val {
            -3 => (Trit::Zero, Trit::Neg),
            -2 => (Trit::Pos, Trit::Neg),
            -1 => (Trit::Neg, Trit::Zero),
            0 => (Trit::Zero, Trit::Zero),
            1 => (Trit::Pos, Trit::Zero),
            2 => (Trit::Neg, Trit::Pos),
            3 => (Trit::Zero, Trit::Pos),
            _ => unreachable!("Trit addition sum out of range"),
        }
    }
}

impl Neg for Trit {
    type Output = Self;
    fn neg(self) -> Self::Output {
        match self {
            Trit::Neg => Trit::Pos,
            Trit::Zero => Trit::Zero,
            Trit::Pos => Trit::Neg,
        }
    }
}

/// A Tryte consists of 9 Trits.
/// Range: -9841 to +9841 (3^9 = 19683 total values)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tryte([Trit; 9]);

impl Tryte {
    pub const ZERO: Tryte = Tryte([Trit::Zero; 9]);

    pub fn new(trits: [Trit; 9]) -> Self {
        Tryte(trits)
    }

    pub fn trits(&self) -> &[Trit; 9] {
        &self.0
    }

    pub fn from_i32(mut val: i32) -> Self {
        let mut trits = [Trit::Zero; 9];
        for i in 0..9 {
            let rem = ((val + 1).rem_euclid(3)) - 1;
            trits[i] = Trit::from_i8(rem as i8).unwrap();
            val = (val - rem) / 3;
        }
        Tryte(trits)
    }

    pub fn to_i32(&self) -> i32 {
        let mut val: i32 = 0;
        let mut power: i32 = 1;
        for trit in self.0 {
            val += (trit.to_i8() as i32) * power;
            power *= 3;
        }
        val
    }
}

impl Neg for Tryte {
    type Output = Self;
    fn neg(self) -> Self::Output {
        let mut result = [Trit::Zero; 9];
        for i in 0..9 {
            result[i] = -self.0[i];
        }
        Tryte(result)
    }
}

impl Add for Tryte {
    type Output = Self;

    fn add(self, other: Self) -> Self::Output {
        let mut result = [Trit::Zero; 9];
        let mut carry = Trit::Zero;
        for i in 0..9 {
            let (s, c) = self.0[i].full_add(other.0[i], carry);
            result[i] = s;
            carry = c;
        }
        Tryte(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trit_add() {
        assert_eq!(Trit::Pos.full_add(Trit::Pos, Trit::Zero), (Trit::Neg, Trit::Pos)); // 1 + 1 = 2 (Sum -1, Carry 1)
        assert_eq!(Trit::Neg.full_add(Trit::Neg, Trit::Zero), (Trit::Pos, Trit::Neg)); // -1 + -1 = -2 (Sum 1, Carry -1)
    }

    #[test]
    fn test_tryte_conversion() {
        let t = Tryte::from_i32(42);
        assert_eq!(t.to_i32(), 42);
        
        let t2 = Tryte::from_i32(-100);
        assert_eq!(t2.to_i32(), -100);
    }

    #[test]
    fn test_tryte_addition() {
        let a = Tryte::from_i32(100);
        let b = Tryte::from_i32(200);
        assert_eq!((a + b).to_i32(), 300);

        let c = Tryte::from_i32(-50);
        assert_eq!((a + c).to_i32(), 50);
    }

    #[test]
    fn test_tryte_negation() {
        let a = Tryte::from_i32(100);
        assert_eq!((-a).to_i32(), -100);
    }
}
