# Preemptive multitasking — roadmap to "DOOM in a window next to the browser"

The standalone milestone (commit 91c18b8) runs id Software's real DOOM on the
pure-Rust kernel — but **fullscreen**, as the only ring-3 task, with the desktop
suspended. Simeon's goal is the real DOOM running **in a window next to the
live browser**, both at once. That needs three things the single-task model
lacks. This is the multi-year "threads/processes" brick, built in small,
**individually boot-verifiable** increments (every test is a QEMU round-trip).

Each increment is gated behind a kernel cmdline flag so it can never break the
working `bin/desktop` boot until it's proven.

## Increment 1 — context-switch primitive  (flag: `schedtest`)  ← IN PROGRESS
Cooperative switching between two **kernel-mode** tasks (same address space,
separate kernel stacks). Proves `context_switch` (save/restore callee-saved +
rsp swap) works. Verified by interleaved serial output. Risk: pure kernel, does
not touch the desktop.

## Increment 2 — timer preemption
Drive the switch from the 100 Hz timer IRQ instead of explicit `yield`. fbDOOM's
game loop never yields, so preemption is mandatory. Switch stacks from inside
the IRQ handler. Verified: two non-yielding kernel loops still interleave.

## Increment 3 — ring-3 tasks + per-process address spaces (VMM)
Each task gets its own CR3 (page tables). Switch CR3 on context switch. Lets the
desktop (loads at 4 MiB + 24 MiB heap) and fbDOOM (16 MiB + brk 112 MiB + mmap
128–256 MiB) use overlapping/fixed virtual addresses without colliding —
otherwise their memory regions overlap in one address space. Verified: desktop
runs as task 0, a second trivial ring-3 ELF as task 1.

## Increment 4 — virtual `/dev/fb0`
When a Linux process opens `/dev/fb0`, hand it an **offscreen** buffer (not the
real hardware FB) sized to a window, and report that geometry via the
`FBIOGET_*` ioctls. DOOM renders into the buffer instead of seizing the screen.

## Increment 5 — composite + input routing
The desktop blits each Linux task's offscreen FB into a window every frame, and
forwards keystrokes to that task's stdin/event path. fbDOOM becomes a normal
desktop window.

## Result
Browser window + real-DOOM window, both live, on our own kernel → the screenshot.
