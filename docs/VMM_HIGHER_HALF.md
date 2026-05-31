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
   - **Sub-step 1a — build the physmap (DONE, VERIFIED).** `build_physmap()` +
     `phys_to_virt()`, additive (coexists with the low identity map). Boot flag
     `physmaptest`: phys `0x100000` read via the identity map and via the physmap
     match (`0xe85250d6`) → "MATCH: higher-half physmap works". Verified in QEMU
     2026-05-31 (sandbox QEMU now available; the whole `schedtest..schedtest5`
     chain was re-confirmed in the same pass).
   - **Sub-step 1b — route page-table frame access through the physmap (DONE,
     VERIFIED 2026-05-31).** `build_physmap` now runs during normal boot (after
     `extend_identity_map`, sized to RAM). A single `frame_virt(phys)` chokepoint
     returns the physmap address once a `PHYSMAP_READY` flag is set (at the end
     of the build) and the low identity address before that; `read64`/`write64`/
     `zero_frame`/`descend`/`new_address_space` all go through it. So every
     paging-structure access after boot uses the higher half — the VMM no longer
     depends on `PML4[0]`. `build_physmap` is idempotent (re-running it would
     install an empty PDPT into `PML4[256]` and #PF the next frame access — found
     and fixed via `physmaptest`). All six flags pass with zero faults.
     Still TODO before the alias drop: the framebuffer pointer (`fb.rs`) and the
     kernel heap/stack still use low identity addresses.
2. Map the kernel image into the higher half **and run from it**; keep the low
   identity map as a temporary alias. Verify: kernel still boots/runs.
   - **DONE, VERIFIED 2026-05-31.** The kernel is now linked at `KERNEL_VMA`
     (`0xFFFFFFFF80000000`, -2 GiB) via a split `linker.ld`: a low-linked boot
     stub (`.boot`/`.boot.bss` — multiboot header, 32-bit entry, page tables,
     early stack) plus the higher-half kernel (`AT()` loads it low). Target gets
     `"code-model": "kernel"`. `boot.s` maps the -2 GiB window (`PML4[511]` →
     `pdpt_high` → `pd_high`, 32×2 MiB → phys 0–64 MiB) alongside the low
     identity map, then `movabs $kernel_main; call *%rax` jumps RIP up. First
     serial line confirms it: `[hh] kernel_main RIP = 0xffffffff80…  (higher
     half OK)`. `pmm` reserves the kernel by physical end (`kernel_end −
     KERNEL_VMA`). All six flags (`physmaptest`, `schedtest`..`schedtest5`) pass
     from the higher half. Framebuffer is still reached via the low identity
     alias — moving it higher-half is deferred to step 3's alias drop.
   - Regression fixed in the same pass: `new_address_space` now copies the
     kernel's higher-half PML4 entries (`[511]` image, `[256]` physmap) in
     addition to `[0]`, or the first kernel instruction after a CR3 switch
     faults (caught it: `schedtest5` fired 0 ring-3 syscalls until fixed → 64).
3. `new_address_space` copies higher-half PML4 entries only; **drop the low
   alias** (move framebuffer + page-frame/heap access to the physmap first, via
   sub-step 1b). Verify (`schedtest3`-style): two address spaces with
   **different low-half** mappings + shared kernel. NOT YET DONE — this is the
   next step. Prereq: 1b (route `read64`/`write64`/`descend` + the FB pointer
   through `phys_to_virt`) so nothing depends on `PML4[0]` anymore.
4. Load the desktop and a second process into private low halves; run both
   ring-3 under the scheduler. Verify: both run, isolated.

Then Increment 4 (virtual `/dev/fb0`) and 5 (compositing) land on a clean base.

## Status (2026-05-31)
Option A chosen and largely DONE — all QEMU-verified in-sandbox:
- **1a** physmap built · **1b** page-table access routed through it.
- **2** kernel relinked to -2 GiB; RIP runs in the higher half.
- **3a** kernel stack via the -2 GiB alias · **3b** framebuffer in the higher
  half (USER-accessible so ring 3 can render) · **3c** initrd via the physmap.
  The kernel no longer needs PML4[0] for stack / FB / initrd / page tables.
- **3d** `new_address_space_private()` shares only the kernel's higher half, so
  each process gets a PRIVATE LOW HALF. Proven by `schedtest6`: two ring-3 tasks
  both at low VA 0x400000 with different payloads, isolated, kernel servicing
  both from address spaces that do NOT map PML4[0].

The real `bin/desktop` boots and renders through the higher-half USER framebuffer
with zero faults; all seven sched/VMM self-test flags pass.

Remaining (Increment 4–5, the windowed-DOOM compositor): load the real desktop
into a private-low AS (needs an ELF loader that maps into a target AS, plus the
remaining device MMIO — AHCI/USB/NIC/HDA — moved high to fully null PML4[0] in
the kernel master AS), a virtual `/dev/fb0` (offscreen buffer per process), and
compositing two ring-3 processes (desktop + DOOM) into windows.
