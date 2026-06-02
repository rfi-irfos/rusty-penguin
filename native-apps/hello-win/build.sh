#!/usr/bin/env bash
# Native Windows App Pipeline
# Target: Rusty Penguin Native Wine Engine

set -euo pipefail

# Minimal build stub: Assumes a cross-compiler is available in the environment
# or is part of the toolchain configuration.
echo "  [build] Compiling hello-win.exe for native Wine..."
x86_64-w64-mingw32-gcc main.c -o hello-win.exe -luser32 -lkernel32 -mwindows

echo "  [build] Native PE binary ready: hello-win.exe"
