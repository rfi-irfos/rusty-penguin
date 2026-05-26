# 🐧 Rusty Penguin

> "Binary hardware. Ternary mind."

Rusty Penguin is a ternary-first operating system built in Rust, running on the Linux kernel. It is architected from day one for future native ternary silicon while providing a transitional layer on conventional binary hardware.

## Core Philosophy

- **-1 (Suppress / Reject)**
- **0 (Dormant / Neutral)**
- **+1 (Active / Promote)**

## Mission

1. **Dormancy Is Sacred**: Aggressive optimization for sparse execution and inactive memory.
2. **Computation Has Direction**: Promoting, ignoring, or suppressing instead of just yes/no.
3. **Hardware Evolution**: Preparing the software substrate for future ternary CPUs.

## Structure

The project is organized into 10 agent-driven tracks:

- `ternary-core`: Core primitives (Trit, Tryte).
- `scheduler`: Ternary process lifecycle management.
- `mathematics`: Balanced ternary arithmetic.
- `shell`: The Penguin Shell (psh).
- ... and more (see `docs/RUSTY_PENGUIN_TERNARY_DIRECTIVE.md`).

## Getting Started

```bash
cargo run -p shell
```
