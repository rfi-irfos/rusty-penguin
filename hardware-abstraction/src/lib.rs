#![no_std]
use ternary_core::{Trit, Tryte};

pub trait TernaryALU {
    fn add(&self, a: Tryte, b: Tryte) -> Tryte;
    fn sub(&self, a: Tryte, b: Tryte) -> Tryte;
    fn transform(&self, a: Tryte, operation: Trit) -> Tryte;
}

pub struct SoftwareALU;

impl TernaryALU for SoftwareALU {
    fn add(&self, a: Tryte, b: Tryte) -> Tryte {
        a + b
    }

    fn sub(&self, a: Tryte, b: Tryte) -> Tryte {
        a + (-b)
    }

    fn transform(&self, a: Tryte, operation: Trit) -> Tryte {
        match operation {
            Trit::Pos => a,
            Trit::Zero => Tryte::ZERO,
            Trit::Neg => -a,
        }
    }
}
