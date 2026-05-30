use core::mem::size_of;

// ── Public selectors ────────────────────────────────────────────────────────
pub const KCODE_SEL: u16 = 0x08;
pub const KDATA_SEL: u16 = 0x10;
pub const UDATA_SEL: u16 = 0x1B;  // 0x18 | RPL=3
pub const UCODE_SEL: u16 = 0x23;  // 0x20 | RPL=3
pub const TSS_SEL:   u16 = 0x28;

// STAR MSR layout for SYSCALL/SYSRET:
//   SYSCALL: CS ← STAR[47:32],   SS ← STAR[47:32]+8
//   SYSRET:  CS ← STAR[63:48]+16|3, SS ← STAR[63:48]+8|3
// With STAR[47:32]=0x08 and STAR[63:48]=0x10:
//   SYSCALL: CS←0x08 (kcode), SS←0x10 (kdata)
//   SYSRET:  CS←0x23 (ucode|3), SS←0x1B (udata|3)
pub const STAR_SYSCALL: u16 = KCODE_SEL;
pub const STAR_SYSRET:  u16 = KDATA_SEL;

// ── 64-bit TSS (104 bytes, IA-32e spec) ─────────────────────────────────────
#[repr(C, packed)]
struct Tss {
    _res0:  u32,
    rsp0:   u64,
    rsp1:   u64,
    rsp2:   u64,
    _res1:  u64,
    ist:    [u64; 7],
    _res2:  u64,
    _res3:  u16,
    iomap:  u16,
}
impl Tss {
    const fn new() -> Self {
        Self { _res0:0, rsp0:0, rsp1:0, rsp2:0, _res1:0, ist:[0;7],
               _res2:0, _res3:0, iomap: size_of::<Self>() as u16 }
    }
}

// ── GDT layout ──────────────────────────────────────────────────────────────
// 0x00 null | 0x08 kcode | 0x10 kdata | 0x18 udata | 0x20 ucode
// 0x28 tss_lo | 0x30 tss_hi
#[repr(C, align(8))]
struct Gdt { entries: [u64; 7] }

#[repr(C, packed)]
struct GdtPtr { limit: u16, base: u64 }

static mut GDT: Gdt = Gdt { entries: [
    0,                      // 0x00  null
    0x00AF_9A00_0000_FFFF,  // 0x08  64-bit kernel code  (P DPL=0 L=1 G=1)
    0x00CF_9200_0000_FFFF,  // 0x10  kernel data          (P DPL=0 DB=1 G=1)
    0x00CF_F200_0000_FFFF,  // 0x18  user data            (P DPL=3 DB=1 G=1)
    0x00AF_FA00_0000_FFFF,  // 0x20  64-bit user code     (P DPL=3 L=1 G=1)
    0,                      // 0x28  TSS low  (runtime)
    0,                      // 0x30  TSS high (runtime)
]};

static mut TSS:    Tss    = Tss::new();
static mut KSTACK: [u8; 65536] = [0; 65536];  // ring-0 interrupt stack

/// Set TSS.rsp0 — the kernel stack the CPU loads on a ring-3→ring-0 interrupt.
/// The scheduler updates this on every switch so a preempted RING-3 task lands
/// its interrupt frame on its OWN kernel stack (else concurrent ring-3 tasks
/// would clobber a single shared stack).
pub fn set_rsp0(top: u64) {
    unsafe { TSS.rsp0 = top; }
}

pub fn init() {
    unsafe {
        TSS.rsp0 = core::ptr::addr_of!(KSTACK) as u64 + 65536;

        let base  = core::ptr::addr_of!(TSS) as u64;
        let limit = (size_of::<Tss>() - 1) as u64;

        // 16-byte TSS system descriptor (type=0x9 = available 64-bit TSS)
        GDT.entries[5] = (limit & 0xFFFF)
            | ((base & 0x00FF_FFFF) << 16)
            | (0x89_u64 << 40)
            | (((limit >> 16) & 0xF) << 48)
            | (((base >> 24) & 0xFF) << 56);
        GDT.entries[6] = (base >> 32) & 0xFFFF_FFFF;

        let ptr = GdtPtr {
            limit: (size_of::<Gdt>() - 1) as u16,
            base: core::ptr::addr_of!(GDT) as u64,
        };

        core::arch::asm!(
            "lgdt [{ptr}]",
            "mov ax, 0x10",
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            "xor eax, eax",
            "mov fs, ax",
            "mov gs, ax",
            "mov ax, 0x28",     // TSS selector
            "ltr ax",
            ptr = in(reg) &ptr,
            out("ax") _,
            options(nostack),
        );
    }
}
