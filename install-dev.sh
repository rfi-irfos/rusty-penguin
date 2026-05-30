#!/usr/bin/env bash
# Rusty Penguin — zero-to-QEMU installer
# Installs all dependencies, builds the ISO, and launches it.
# Tested on Ubuntu 22.04+, Debian 12+, Fedora 38+, and macOS 14+.
set -euo pipefail

REPO="https://github.com/rfi-irfos/rusty-penguin"
DIR="${RUSTY_PENGUIN_DIR:-$HOME/rusty-penguin}"

echo "==> Rusty Penguin dev setup"

# ── 1. Rust nightly + bare-metal targets ────────────────────────────────────
if ! command -v rustup &>/dev/null; then
    echo "--> Installing Rust toolchain..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly
    source "$HOME/.cargo/env"
fi
echo "--> Ensuring nightly toolchain and rust-src..."
rustup toolchain install nightly --component rust-src 2>/dev/null || true
rustup default nightly

# ── 2. System packages ───────────────────────────────────────────────────────
if [[ "$OSTYPE" == "darwin"* ]]; then
    if ! command -v brew &>/dev/null; then
        echo "--> Installing Homebrew..."
        /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    fi
    brew install qemu xorriso grub 2>/dev/null || true
elif command -v apt-get &>/dev/null; then
    echo "--> Installing packages via apt..."
    sudo apt-get update -qq
    sudo apt-get install -y qemu-system-x86 grub-pc-bin grub-efi-amd64-bin xorriso mtools
elif command -v dnf &>/dev/null; then
    echo "--> Installing packages via dnf..."
    sudo dnf install -y qemu-system-x86 grub2-tools xorriso
elif command -v pacman &>/dev/null; then
    echo "--> Installing packages via pacman..."
    sudo pacman -Sy --noconfirm qemu-system-x86 grub xorriso
else
    echo "WARN: unknown package manager — install qemu-system-x86_64, grub-mkrescue, xorriso manually"
fi

# ── 3. Clone or update repo ──────────────────────────────────────────────────
if [ -d "$DIR/.git" ]; then
    echo "--> Updating repo at $DIR..."
    git -C "$DIR" pull --ff-only
else
    echo "--> Cloning $REPO into $DIR..."
    git clone "$REPO" "$DIR"
fi

# ── 4. Build ISO ─────────────────────────────────────────────────────────────
echo "--> Building Rusty Penguin ISO (takes ~2 minutes)..."
cd "$DIR"
bash iso/build.sh

# ── 5. Launch ────────────────────────────────────────────────────────────────
echo ""
echo "==> Build complete: rusty-penguin.iso"
echo "==> Launching QEMU..."
bash launch.sh
