/* boot.s — Rusty Penguin bare-metal boot stub
 *
 * GRUB loads this in 32-bit protected mode (Multiboot2 spec).
 * We set up minimal 2MB identity-map page tables, enable long mode,
 * and jump to the 64-bit Rust kernel_main.
 *
 * AT&T syntax, assembled with: as --64
 */

/* ── Multiboot2 header ────────────────────────────────────────────────────── */
    .section .multiboot2, "a"
    .align 8
mb2_start:
    .long   0xE85250D6              /* magic */
    .long   0                       /* arch: i386 protected mode */
    .long   mb2_end - mb2_start     /* header length */
    /* checksum: -(magic + arch + length) truncated to 32 bits */
    .long   -(0xE85250D6 + 0 + (mb2_end - mb2_start))
    /* framebuffer request tag (type=5): ask GRUB for a linear framebuffer.
     * Request a high resolution explicitly — leaving width/height 0 makes GRUB
     * fall back to its default 800x600x24. GRUB matches the closest mode the
     * firmware/GOP supports; flags=1 (optional) keeps us booting if it can't. */
    .align  8
    .short  5                       /* type */
    .short  1                       /* flags: 1 = optional (still boot if unsupported) */
    .long   20                      /* size: 20 bytes */
    .long   1920                    /* preferred width  */
    .long   1080                    /* preferred height */
    .long   32                      /* preferred bpp */

    /* end tag */
    .align  8
    .short  0
    .short  0
    .long   8
mb2_end:

/* ── BSS: stack + page tables ────────────────────────────────────────────── */
    .section .bss
    .align 16
stack_bottom:
    .skip   65536               /* 64 KB kernel stack */
stack_top:

    .align 4096
pml4_table:
    .skip   4096
pdpt_table:
    .skip   4096
pd_table:
    .skip   4096

/* ── 32-bit entry point ──────────────────────────────────────────────────── */
    .section .text
    .code32
    .global _start
_start:
    /* Disable interrupts, set up temporary stack */
    cli
    movl    $stack_top, %esp

    /* Save multiboot2 magic (EAX) and info pointer (EBX) for later */
    movl    %eax, %edi          /* arg0 to kernel_main: magic */
    movl    %ebx, %esi          /* arg1 to kernel_main: mb2 info addr */

    /* ── Set up page tables for 2MB identity map ── */

    /* PML4[0] → pdpt_table (present + writable) */
    movl    $pdpt_table, %eax
    orl     $0x3, %eax
    movl    %eax, pml4_table

    /* PDPT[0] → pd_table (present + writable) */
    movl    $pd_table, %eax
    orl     $0x3, %eax
    movl    %eax, pdpt_table

    /* PD[0..31] → 32 × 2MB huge pages = 64MB identity map
     * GRUB can place the MB2 info struct past 2MB when a framebuffer tag is
     * present, so we need more than the initial single 2MB page. */
    xorl    %ecx, %ecx
.Lmap_pd:
    movl    %ecx, %eax
    shll    $21, %eax           /* physical addr = index * 2MB */
    orl     $0x83, %eax         /* present + writable + huge */
    movl    %ecx, %edx
    shll    $3, %edx            /* byte offset = index * 8 bytes/entry */
    movl    %eax, pd_table(%edx) /* low 32 bits (high 32 bits already 0) */
    incl    %ecx
    cmpl    $32, %ecx           /* 32 entries × 2MB = 64MB */
    jl      .Lmap_pd

    /* ── Enable PAE (Physical Address Extension) ── */
    movl    %cr4, %eax
    orl     $(1 << 5), %eax     /* CR4.PAE */
    movl    %eax, %cr4

    /* ── Load CR3 with PML4 address ── */
    movl    $pml4_table, %eax
    movl    %eax, %cr3

    /* ── Enable long mode in EFER MSR ── */
    movl    $0xC0000080, %ecx   /* EFER MSR number */
    rdmsr
    orl     $(1 << 8), %eax     /* EFER.LME */
    wrmsr

    /* ── Enable paging (and protected mode — already set by GRUB) ── */
    movl    %cr0, %eax
    orl     $(1 << 31), %eax    /* CR0.PG */
    movl    %eax, %cr0
    /* CPU is now in 64-bit compatibility mode */

    /* ── Load 64-bit GDT ── */
    lgdt    gdt64_ptr

    /* ── Far jump to flush pipeline and enter 64-bit mode ── */
    ljmp    $0x08, $long_mode_start

/* ── Minimal 64-bit GDT (code + data) ───────────────────────────────────── */
    .align 8
gdt64:
    .quad   0                   /* null descriptor */
    /* code segment: 64-bit, present, DPL=0, execute/read */
    .quad   0x00AF9A000000FFFF
    /* data segment: 64-bit, present, DPL=0, read/write */
    .quad   0x00AF92000000FFFF
gdt64_ptr:
    .short  gdt64_ptr - gdt64 - 1
    .long   gdt64

/* ── 64-bit entry ────────────────────────────────────────────────────────── */
    .code64
long_mode_start:
    /* Reload data segment registers */
    movw    $0x10, %ax
    movw    %ax, %ss
    movw    %ax, %ds
    movw    %ax, %es
    movw    %ax, %fs
    movw    %ax, %gs

    /* Restore stack pointer (still valid from 32-bit setup) */
    movq    $stack_top, %rsp

    /* EDI/ESI already hold magic and mb2 info from 32-bit code above.
     * In 64-bit mode, RDI = arg0, RSI = arg1 (System V ABI).
     * Zero-extend to 64-bit for cleanliness. */
    movl    %edi, %edi          /* zero-extends to RDI */
    movl    %esi, %esi          /* zero-extends to RSI */

    /* Call Rust kernel entry point */
    call    kernel_main

    /* Should never return — halt if it does */
halt_loop:
    hlt
    jmp     halt_loop
