//! Trit-TMR: Ternary Triple Modular Redundancy.
//! Hardens kernel memory against radiation and hardware-level SEUs (Single Event Upsets)
//! by performing consensus voting on three redundant trit-states.

use crate::serial;

/// Consensus voting: Given three trits, return the majority state.
/// If all disagree, return 0 (Dormant/Safe state).
pub fn vote(t1: i8, t2: i8, t3: i8) -> i8 {
    if t1 == t2 || t1 == t3 { return t1; }
    if t2 == t3 { return t2; }
    0 // Error state -> Dormant (Safe)
}

/// Hardened read of a ternary memory word.
pub fn hardened_read(addr: *const [i8; 3]) -> i8 {
    unsafe {
        let tri = *addr;
        vote(tri[0], tri[1], tri[2])
    }
}

/// Hardened write of a ternary memory word.
pub fn hardened_write(addr: *mut [i8; 3], val: i8) {
    unsafe {
        *addr = [val, val, val];
    }
}
