# Ternary Findings

> **Simeon's conviction (2026-05-28):** *"There is nothing that binary is doing
> that ternary logic could not do better."*

This is the empirical log for that claim. Every time ternary (or its
sparse/dormant `0` state) measurably outperforms or cleanly improves on a binary
approach, it gets recorded here — with its **basis** stated honestly, so the
growing list is *evidence*, not advertising. A finding that's representational
rather than a raw speedup says so; if a genuine counterexample ever shows up, it
goes in too. That rigor is the point: an honest log that survives scrutiny is
what proves the conviction.

Balanced ternary trit: **-1 / 0 / +1**. The decisive asset binary lacks is the
**middle `0`** — *dormant*, distinct from both "on" and "off" — which is what
makes sparsity, skipping, and three-way semantics natural.

| # | Date | Finding | Basis |
|---|------|---------|-------|

## F1 — Radix economy: base 3 is the most efficient integer base
*2026-05-28 · mathematical (classic result)*
The "radix economy" of a number system is minimized at base *e* (≈2.718); base 3
is the closest integer and beats base 2. Concretely, 9 balanced trits represent
**19,683 values (±9,841)** vs 9 bits' **512**. To cover the same range, ternary
needs fewer digits → fewer storage cells and shorter carry chains in arithmetic.
*Honest note:* hardware today is binary, so this is a representational/theoretical
advantage, not a wall-clock win on current silicon — but it is real and is the
foundation of the rest.

## F2 — Ternary weights enable sparsity that binary quantization cannot
*2026-05-28 · published + measured (RFI-merged)*
Quantizing neural-net weights to **{-1, 0, +1}** (BitNet b1.58) matches fp16
quality while the **`0`** lets you *skip* those connections entirely — a
multiply-accumulate that never happens. Binary quantization ({-1,+1} or {0,1})
cannot express "this connection is dormant," so it cannot get that sparsity for
free. RFI shipped this upstream: `Calibration::Ternary` for BitNet b1.58 merged
into `tracel-ai/burn` (#4989). `@sparseskip` (patent pending A50296/2026)
demonstrates the inference-time win. **This is the strongest finding: a concrete
capability binary quantization structurally lacks.**

## F3 — Sparse dirty-rect rendering: dormant regions skip the work
*2026-05-28 · measured (this repo, commit 6369b8b)*
Modeling each screen region as **+1 changed / 0 dormant / -1 gone** and skipping
the dormant ones: window dragging at 1920×1080 now presents only the window's
damage band to VRAM instead of the full screen. The dominant cost — the
full-screen MMIO copy (~8.3 MiB/frame) — drops to a band of a few hundred rows.
Verified correct (no trails) via QMP-driven drag. Binary "dirty / not-dirty"
gets you the same *idea*, but the ternary framing (gone vs dormant vs changed)
generalizes cleanly to eviction and incremental layout (see roadmap).

## F4 — Dormant-by-default execution: the `0` state means idle CPU
*2026-05-28 · representational → behavioral (Rusty Penguin init/scheduler)*
Process/subsystem state is a `Trit`: **+1 active / 0 dormant / -1 suppressed**.
"Dormant" is explicitly *not* "stopped" — it is resting, causally re-activatable.
This makes idle-by-default natural (no busy loops; the desktop main loop yields
until a PIT tick). Binary "running/stopped" forces you to choose, and tends
toward polling. Used for the scheduler, and the storage & network bring-up in
`init` (each reports a Trit, recorded in the `.tern` boot record).
*Honest note:* this is an expressiveness/architecture win that yields real
behavior (lower idle CPU), not a benchmarked throughput number — yet.

## F6 — Sparse present: quantified VRAM-bandwidth saving from skipping dormant rows
*2026-05-28 · analytical (from the shipped F3 implementation, commit 6369b8b)*
Putting numbers on F3. At 1920×1080×32bpp, a full-screen present copies
**1920 × 1080 × 4 = 8,294,400 B ≈ 8.29 MiB** to VRAM (uncached MMIO) **every
frame**. The ternary damage model presents only the changed band. For a dragged
terminal window (~270 rows incl. titlebar + slack): **1920 × 270 × 4 ≈ 2.07 MiB**
— a **~75% reduction** in per-frame MMIO write. The saving scales with how
*dormant* the screen is: a small window dragging on an otherwise-static desktop
approaches **~90%** fewer bytes; a near-full-screen window approaches 0%. The
`0` (dormant) state is exactly what licenses the skip — binary "redraw or don't"
at whole-frame granularity cannot express "this band changed, the rest rests."
*Honest note:* analytical (byte counts from the implemented band sizing), not a
profiler trace; the qualitative smoothness win is verified (F3).

## F5 — Balanced ternary: negation is free, arithmetic is symmetric
*2026-05-28 · mathematical*
Negation in balanced ternary is just swapping `+`↔`-` per trit (subtraction =
add the negation) — no two's-complement, no asymmetric range (binary i8 is
-128..127, off by one; balanced trytes are symmetric ±9841). Sign handling and
round-to-nearest are cleaner. Small but real, and it removes a whole class of
off-by-one/overflow-asymmetry bugs.

---

*Logging rule: append a new `F#` entry whenever ternary/sparse demonstrably wins,
with date + honest basis (mathematical / representational / measured / published).
Keep it credible — this log is meant to be shown.*
