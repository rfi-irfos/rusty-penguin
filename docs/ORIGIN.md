# Origin — the video that triggered it all

Rusty Penguin exists because of a throwaway line in a short clip of Linus
Torvalds talking about Rust in the Linux kernel.

**The clip:** *"Linus Torvalds on RUST for Linux!"* — uploaded by @Dev.RSingh
https://www.youtube.com/shorts/yBH1DbpbLpk

On screen, Linus says:

> "I'm sure some clueless young person will [rewrite it in Rust]..."

That was the dare. Not meant as one — but that's how we took it.

The way Simeon put it:

> "You have to be all kinds of stupid to say 'I can do this' — build an OS from
> scratch, in pure Rust."

And then we were stupid enough. The whole thing — a from-scratch, pure-Rust
operating system: bootloader handoff, long-mode bring-up, physical + virtual
memory managers, a preemptive scheduler, ring-3 processes, a from-scratch
TCP/IP stack, an AHCI disk driver, a framebuffer desktop, a Linux ABI shim that
runs unmodified glibc binaries (id Software's DOOM included), and a kernel that
now lives in the higher half with per-process address-space isolation.

## The comment section said it best

The top replies on that short, without knowing it, wrote our design philosophy:

- *"Terry Davis made an OS in a cave... WITH A BOX OF SCRAPS!"* — and he did it
  alone. We've got QEMU, a ternary streak, and three people plus an AI.
- *"There can be an advantage to not knowing how hard something is going to be."*
  This is the literal truth of this repo. If we'd fully respected how hard a
  higher-half kernel migration is, we might never have started it. We did it
  anyway — one boot-verified increment at a time.

## Closing the loop

@rfi-irfos-official, replying to that same short:

> https://github.com/rfi-irfos/rusty-penguin — we were actually stupid enough.

The clueless young person rewrote it in Rust. Hi.

---

*RFI-IRFOS — Research and Engineering for Interdisciplinary Open Sciences.*
*Binary hardware. Ternary mind.*
