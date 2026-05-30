# VMM design decision: isolating the desktop and fbDOOM at low addresses

*Drafted 2026-05-30 night for morning review. This is analysis + a recommendation,
not committed code — pick a direction before we build Increment 3d.*

## The problem

Increments 3a/3b gave us per-process address spaces (`new_address_space` +
per-task CR3 switching, both verified). But `new_address_space` **shares all of
`PML4[0]`** (the kernel's low 512 GiB) with every process. That means:

- Anything below 512 GiB is the **same** in every address space.
- The ring-3 3c stub works only because it lives at **≥512 GiB** (`PML4[1]`,
  private).

The real processes don't:

| | virtual addresses used |
|---|---|
| desktop  | code ~4 MiB, .bss/heap to ~28 MiB, stack ~63 MiB |
| fbDOOM    | code 16 MiB, ld.so 8 MiB, stack 63 MiB, brk 112 MiB, mmap 128–256 MiB |

Both want **low** addresses, and they overlap (stack at 63 MiB; desktop heap vs
fbDOOM code). With `PML4[0]` shared, they cannot have different mappings there —
so they'd corrupt each other. We need the **low half private per process** while
the **kernel stays mapped in every process** (the kernel runs on the process's
CR3 during syscalls/interrupts).

## Options

### A. Higher-half kernel (the standard, "proper" design) — RECOMMENDED
Relocate the kernel's own mappings into the **higher half** (`PML4[256..512]`,
i.e. `0xFFFF_8000_0000_0000+`), shared across all address spaces. User processes
own the **entire lower half** (`PML4[0..256]`) privately.

- `new_address_space`: copy the kernel's **higher-half** PML4 entries (256..512),
  leave the lower half empty/private.
- Kernel code/data/stack/framebuffer get higher-half virtual addresses.
- Each process maps its program/heap/stack in the private lower half — desktop
  at 4 MiB and fbDOOM at 16 MiB no longer collide (different page tables).
- **Cost:** the kernel currently runs identity-mapped low (boot.s sets
  `PML4[0]`). Moving it higher-half means: map kernel into the higher half,
  switch RIP/RSP/pointers to higher-half (or keep a low identity alias during
  transition), and audit code that assumes virt==phys (the PMM/page-table code
  reads frames via their physical address — keep a higher-half **physmap** or a
  low identity alias for that). This is the biggest single change but it's the
  textbook foundation every real OS has; it's the honest "do it properly" path
  and unblocks everything after.

### B. Selective sharing — share only the kernel's PD entries, not all of PML4[0]
Keep the kernel low, but in `new_address_space` share only the specific lower
page-table entries that cover the kernel + framebuffer + PMM physmap, and give
each process private entries elsewhere in the low half.

- **Cost:** fragile. The kernel identity map uses 2 MiB huge pages in one shared
  PD; sharing at PD granularity means processes can't remap those 2 MiB regions,
  and the desktop/fbDOOM addresses sit inside exactly that range. You'd have to
  carve the low half into "kernel-reserved" vs "user" sub-ranges and relocate
  one of the processes out of the kernel-reserved band — i.e. re-introduce the
  address juggling we're trying to avoid. Works for a demo, but it's the "fast"
  path that needs redoing. Conflicts with the "always proper" rule.

### C. Don't isolate — one shared space, relocate fbDOOM's regions
Skip per-process page tables entirely; run desktop + fbDOOM in one address space
with fbDOOM relocated to non-overlapping addresses (e.g. everything +256 MiB).

- **Cost:** no memory protection between processes; a fbDOOM bug corrupts the
  desktop. Also fbDOOM's brk/mmap base addresses are baked into linux.rs and the
  auxv; relocating them is doable but, again, throwaway. Fastest to a screenshot,
  least "proper". Explicitly against the standing decree.

## Recommendation

**Option A (higher-half kernel).** It's the foundation every increment past 3c
rests on, it's the honest multi-year-ABI path, and it removes the address-collision
problem permanently instead of juggling around it. Plan, in verifiable sub-steps
(each gated behind a boot flag, serial-checked, since QEMU runs on Simeon's side):

1. Add a higher-half **physmap** (map all RAM at `0xFFFF_8000_0000_0000 + phys`)
   and switch the PMM/page-table code to access frames through it (instead of
   raw physical = identity). Verify: existing tests still pass.
2. Map the kernel image + framebuffer into the higher half; keep the low identity
   map as a temporary alias. Verify: kernel still boots/runs.
3. `new_address_space` copies higher-half PML4 entries only; drop the low alias.
   Verify (`schedtest3`-style): two address spaces with **different low-half**
   mappings + shared kernel.
4. Load the desktop and a second process into private low halves; run both
   ring-3 under the scheduler. Verify: both run, isolated.

Then Increment 4 (virtual `/dev/fb0`) and 5 (compositing) land on a clean base.

## Open question for Simeon
Higher-half is a meaningful kernel surgery (touches boot, PMM, every `virt==phys`
assumption). Worth doing now for the proper foundation, or do you want a quick
Option-C demo of windowed-DOOM-next-to-browser FIRST (throwaway), then the proper
VMM after SPRIND? Your "always proper" rule says A; flagging the time tradeoff so
it's your call, not mine.
