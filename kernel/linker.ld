/* Higher-half kernel layout (docs/VMM_HIGHER_HALF.md).
 *
 * The boot stub (multiboot2 header, 32-bit entry, page tables, early stack)
 * stays linked at LOW physical addresses — GRUB enters it in 32-bit protected
 * mode before paging, so its symbols must be reachable as physical addresses
 * (VMA == LMA). Everything else (the Rust kernel) is linked into the higher
 * half at -2 GiB (0xFFFFFFFF80000000+) and loaded low via AT(); boot.s maps the
 * -2 GiB window and jumps RIP there. The low identity map is retained as an
 * alias so existing low-address accesses (framebuffer, user programs, page
 * frames) keep working during the migration. */

KERNEL_VMA = 0xFFFFFFFF80000000;

ENTRY(_start)
OUTPUT_FORMAT(elf64-x86-64)

SECTIONS {
    . = 0x100000;

    /* ── Low-linked boot stub (VMA == LMA == physical) ──────────────────────── */
    .boot : ALIGN(8) {
        KEEP(*(.multiboot2))
        *(.boot.text)
    }
    .boot.bss (NOLOAD) : ALIGN(4096) {
        *(.boot.bss)
    }

    /* ── Higher-half kernel: VMA at -2 GiB, LMA continues low via AT() ───────── */
    . += KERNEL_VMA;

    .text : AT(ADDR(.text) - KERNEL_VMA) ALIGN(4096) {
        *(.text .text.*)
    }

    .rodata : AT(ADDR(.rodata) - KERNEL_VMA) ALIGN(4096) {
        *(.rodata .rodata.*)
    }

    .data : AT(ADDR(.data) - KERNEL_VMA) ALIGN(4096) {
        *(.data .data.*)
    }

    .bss : AT(ADDR(.bss) - KERNEL_VMA) ALIGN(4096) {
        bss_start = .;
        *(COMMON)
        *(.bss .bss.*)
        bss_end = .;
    }

    /* kernel_end is a higher-half VMA; pmm wants the physical end, so callers
     * subtract KERNEL_VMA (see main.rs). */
    kernel_end = .;
}
