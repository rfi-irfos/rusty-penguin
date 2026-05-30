# Contributing to Rusty Penguin

Rusty Penguin is built by RFI-IRFOS. We welcome contributions that advance
the project's goal: a complete daily-driver OS written from scratch in pure Rust,
with ternary logic as a first-class primitive at every layer.

## Guiding principles

- **Honest over aspirational.** Don't mark something ✅ if it isn't verified.
- **Ternary first.** Every new subsystem should model its state as `+1/0/−1`
  rather than a boolean where a third state is meaningful.
- **No regressions.** The bare-metal desktop must boot in QEMU after every
  commit. Run `bash launch.sh` to verify.
- **Small bricks.** Each commit should close one verifiable unit of work.
  Big PRs are hard to review and easy to regress.
- **No dead code.** If something isn't used, remove it. The codebase should
  reflect the shipped system, not aspirational futures.

## Development setup

```bash
git clone https://github.com/rfi-irfos/rusty-penguin
cd rusty-penguin

# Install prerequisites (Ubuntu/Debian)
sudo apt install grub-pc-bin grub-efi-amd64-bin xorriso mtools \
     qemu-system-x86 nasm rustup

# Install nightly Rust toolchain
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly

# Build the ISO
bash iso/build.sh

# Run in QEMU
bash launch.sh
```

## Project structure

```
kernel/          Pure-Rust bare-metal kernel (no_std, no libc)
desktop-metal/   Ring-3 desktop compositor + all GUI apps
user-psh/        Bare-metal ring-3 shell (psh)
init/            Linux-track init (PID 1 for the Linux boot entries)
installer/       rp-install — install to disk
iso/             Build scripts, GRUB config, initramfs construction
docs/            Architecture docs, ternary findings, design assets
ternary-core/    Ternary primitives (Trit, Tryte) used across crates
```

## Submitting changes

1. Fork the repo and create a branch.
2. Make your change, verify QEMU boot, run `cargo test` in relevant crates.
3. Open a PR. Describe what the change does and why. Include a serial log
   or screenshot if it touches boot/rendering/networking.
4. A maintainer will review and merge.

## Contact

Issues and PRs: https://github.com/rfi-irfos/rusty-penguin  
Email: rfi.irfos@gmail.com  
Organization: https://rfi-irfos.org  

RFI-IRFOS is a regulated not-for-profit (ZVR 1015608684 · GISA 39261441 ·
Steuernummer 68 028/0989). At least 90% of surplus is reinvested per statute.
