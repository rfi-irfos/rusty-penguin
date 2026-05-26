# Changelog — Rusty Penguin

All notable changes to this project will be documented here.

## [1.0.0] — 2026-05-26

### Initial release — "Binary hardware. Ternary mind."

This is the founding commit of Rusty Penguin, a ternary-first operating system
initiative written entirely in Rust.

#### What works today (userspace personality track)

- **psh** — Penguin Shell (v0.2): interactive REPL that runs as PID 1
  - `trit <n>` — convert any integer to balanced ternary Tryte representation
  - `mul <a> <b>` — double-width balanced ternary multiply
  - `div <a> <b>` — ternary integer division with remainder
  - `scale <n> <trit>` — one-trit conditional transform (Pos/Zero/Neg)
  - `ps` — list real Linux processes annotated with ternary state (+1/0/-1)
  - `activate / dormant / suppress <pid>` — send SIGCONT / SIGSTOP / SIGTERM
  - `ai [n]` — sparse ternary inference layer demo (skip-Zero efficiency)
  - `help / exit`

- **ternary-core** — Trit and Tryte primitives (range ±9841, 9 trits)

- **mathematics** — balanced ternary arithmetic: mul, div, mod, abs, consensus, any, scale

- **scheduler** — ternary process state model; real `/proc` scanning;
  heuristic: R/high-mem-S = Active (+1), idle-S/I = Dormant (0), Z/X = Suppressed (-1)

- **ai-runtime** — TernaryLinear sparse dot-product layer; Zero-dormancy skipping

- **init** — PID 1 process: mounts /proc /sys /dev /tmp, sets hostname, spawns psh

- **iso/build.sh** — grub-mkrescue pipeline to produce a bootable ISO using the
  host Linux kernel + a minimal initramfs containing only the init binary

#### Architecture — two parallel tracks

| Track | Status | Description |
|---|---|---|
| Userspace personality | Active | psh as /sbin/init on stock Linux kernel |
| Bare-metal kernel | Planned | x86_64-unknown-none no_std Rust kernel |

#### What comes next (Phase 2)

- `compiler/` — import ternlang-core lexer/parser/BET bytecode/VM
- `filesystem/` — ternary-annotated VFS layer
- `ipc/` — actor model (adapt ternlang-runtime TernNode to Unix sockets)
- `memory/` — TernPage: ternary-annotated memory pages via mmap/mprotect

---

Active research and development is ongoing. This repository tracks the full
Ternary Intelligence Stack (TIS) OS initiative.

Links:
- Ternary Intelligence Stack: https://ternlang.com
- TIS monorepo: https://github.com/rfi-irfos/ternary-intelligence-stack
