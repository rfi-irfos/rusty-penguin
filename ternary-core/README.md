# ternary-core

Balanced-ternary primitives in `no_std` Rust, with zero dependencies.

Balanced ternary represents numbers in base 3 using the digits `{-1, 0, +1}`
instead of `{0, 1, 2}`. It is symmetric (negation is per-digit), needs no sign
bit, and rounds by truncation. This crate provides the two building blocks used
by the [Rusty Penguin](https://github.com/rfi-irfos/rusty-penguin) operating
system and the wider Ternary Intelligence Stack.

## Types

- **`Trit`** — a single balanced-ternary digit (`Neg = -1`, `Zero = 0`,
  `Pos = +1`). Supports negation and a `full_add` that returns `(sum, carry)`,
  the ternary analogue of a binary full adder.
- **`Tryte`** — nine trits (`3^9 = 19683` values, range `-9841 ..= +9841`).
  Converts to and from `i32`, negates, and adds with ripple carry.

## Example

```rust
use ternary_core::{Trit, Tryte};

// A single trit full-adder: 1 + 1 = 2  ->  sum -1, carry +1
assert_eq!(Trit::Pos.full_add(Trit::Pos, Trit::Zero), (Trit::Neg, Trit::Pos));

// Trytes round-trip through i32 and add with ripple carry.
let a = Tryte::from_i32(100);
let b = Tryte::from_i32(200);
assert_eq!((a + b).to_i32(), 300);

// Negation is per-digit — no sign bit, no two's complement.
assert_eq!((-a).to_i32(), -100);
```

## `no_std`

The crate is `#![no_std]` and pulls in nothing but `core`. It runs on bare
metal, embedded targets, and inside kernels.

## License

MIT © Simeon Kepp / RFI-IRFOS
