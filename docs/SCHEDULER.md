# Preemptive multitasking — roadmap to "DOOM in a window next to the browser"

The standalone milestone (commit 91c18b8) runs id Software's real DOOM on the
pure-Rust kernel — but **fullscreen**, as the only ring-3 task, with the desktop
suspended. Simeon's goal is the real DOOM running **in a window next to the
live browser**, both at once. That needs three things the single-task model
lacks. This is the multi-year "threads/processes" brick, built in small,
**individually boot-verifiable** increments (every test is a QEMU round-trip).

Each increment is gated behind a kernel cmdline flag so it can never break the
working `bin/desktop` boot until it's proven.

## Increment 1 — context-switch primitive  (flag: `schedtest`)  ✅ DONE (2e88425)
Cooperative switching between two **kernel-mode** tasks (same address space,
separate kernel stacks). Proves `context_switch` (save/restore callee-saved +
rsp swap) works. Verified: boot/A/B interleave 3 cycles, clean exit.

## Increment 2 — timer preemption  (flag: `schedtest2`)  ✅ DONE (90ab9c1)
100 Hz timer IRQ switches tasks (full GPR + iret-frame save/restore in a naked
stub) — fbDOOM's loop never yields, so preemption is mandatory. Verified: two
NON-yielding kernel loops + boot thread, 42 prints each, interleaved, no fault.

## Increment 3a — per-process address spaces (VMM)  (flag: `schedtest3`)  ✅ DONE (ac57559)
`new_address_space` (PML4 sharing the kernel low half), `map_page_in`,
`switch_address_space`. Verified: same vaddr (512 GiB) reads 0xbbbb in the
kernel AS, 0xaaaa in a second AS, no fault on CR3 switch.

## Increment 3b — per-task CR3 switching under preemption  (flag: `schedtest4`)  ✅ DONE (0010f99)
`preempt_tick` swaps CR3 per task; `spawn_preempt_as` spawns into a fresh AS.
Verified: boot/A/B each print a distinct live CR3 (0x22f000 / 0x238000 /
0x239000), interleaved, no fault. **Foundation complete: preemptive multitasking
+ per-process isolation.**

## Increment 3c — ring-3 task in a private address space  ← NEXT
So far tasks run in ring 0. A real process (desktop, fbDOOM) runs in ring 3.
Needs: load an ELF into a private AS (PMM frames mapped USER at its vaddrs),
a user stack, a ring-3 iret frame (cs=0x23/ss=0x1b), AND **per-task TSS.rsp0**
— when the timer interrupts a ring-3 task, the CPU loads the kernel stack from
TSS.rsp0, so `preempt_tick` must update rsp0 to the next task's kernel stack on
every switch (else a ring-3 preemption corrupts state). Verify: a ring-3 stub
makes a syscall the kernel logs, preempted alongside the boot thread.

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
