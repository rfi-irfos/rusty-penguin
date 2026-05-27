// Balanced ternary primitives — inline sparse AI inference for desktop-metal.
// No external crate deps: avoids workspace-key resolution across standalone workspaces.

#[derive(Clone, Copy, PartialEq)]
#[repr(i8)]
pub enum Trit { Neg = -1, Zero = 0, Pos = 1 }

impl Trit {
    #[inline]
    pub fn to_byte(self) -> u8 {
        match self { Trit::Pos => b'+', Trit::Zero => b'0', Trit::Neg => b'-' }
    }
}

/// Sparse dot product — skips any pair where either operand is Zero.
/// Returns (sum, total_pairs, skipped_pairs).
pub fn sparse_dot(a: &[Trit], b: &[Trit]) -> (i32, usize, usize) {
    let len = a.len().min(b.len());
    let mut sum = 0i32;
    let mut skip = 0usize;
    for i in 0..len {
        match (a[i], b[i]) {
            (Trit::Zero, _) | (_, Trit::Zero) => skip += 1,
            (Trit::Pos, Trit::Pos) | (Trit::Neg, Trit::Neg) => sum += 1,
            _ => sum -= 1,
        }
    }
    (sum, len, skip)
}

/// Dense ternary linear layer: W[rows×cols] × x[cols] → out[rows].
/// Activation: sign of weighted sum (Pos/Neg/Zero).
/// Returns (total_ops, skipped_ops).
pub fn linear_layer(w: &[Trit], rows: usize, cols: usize, x: &[Trit], out: &mut [Trit]) -> (usize, usize) {
    let mut total = 0usize;
    let mut skip  = 0usize;
    for r in 0..rows {
        let (s, t, sk) = sparse_dot(&w[r * cols..(r + 1) * cols], &x[..cols]);
        out[r] = if s > 0 { Trit::Pos } else if s < 0 { Trit::Neg } else { Trit::Zero };
        total += t;
        skip  += sk;
    }
    (total, skip)
}

/// LCG-based pseudo-random trit seeder.  Used to init weights + activations.
pub fn seed_trits(buf: &mut [Trit], mut s: u64) {
    for t in buf.iter_mut() {
        s = s.wrapping_mul(6_364_136_223_846_793_005)
             .wrapping_add(1_442_695_040_888_963_407);
        *t = match (s >> 33) % 3 {
            0 => Trit::Neg,
            1 => Trit::Zero,
            _ => Trit::Pos,
        };
    }
}
