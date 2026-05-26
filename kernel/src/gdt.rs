use core::mem::size_of;

// 64-bit TSS — 104 bytes per IA-32e spec
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
        Self {
            _res0: 0, rsp0: 0, rsp1: 0, rsp2: 0,
            _res1: 0, ist: [0; 7], _res2: 0, _res3: 0,
            iomap: size_of::<Self>() as u16,
        }
    }
}

// GDT layout: null(0x00) | kcode(0x08) | kdata(0x10) | tss_lo(0x18) | tss_hi(0x20)
#[repr(C, align(8))]
struct Gdt { entries: [u64; 5] }

#[repr(C, packed)]
struct GdtPtr { limit: u16, base: u64 }

static mut GDT: Gdt = Gdt { entries: [
    0,                      // 0x00  null
    0x00AF_9A00_0000_FFFF,  // 0x08  64-bit kernel code (P=1 DPL=0 L=1 G=1)
    0x00CF_9200_0000_FFFF,  // 0x10  kernel data         (P=1 DPL=0 DB=1 G=1)
    0,                      // 0x18  TSS low  (filled at runtime)
    0,                      // 0x20  TSS high (filled at runtime)
]};

static mut TSS: Tss = Tss::new();

// 64 KiB kernel stack for ring-0 interrupt frames
static mut KSTACK: [u8; 65536] = [0; 65536];

pub fn init() {
    unsafe {
        // Point RSP0 to top of our kernel interrupt stack
        TSS.rsp0 = core::ptr::addr_of!(KSTACK) as u64 + 65536;

        let base  = core::ptr::addr_of!(TSS) as u64;
        let limit = (size_of::<Tss>() - 1) as u64;

        // 16-byte system descriptor for 64-bit TSS (type = 0x9 = available)
        // Low 8 bytes: [15:0]=limit_lo, [39:16]=base_lo, [47:40]=0x89, [51:48]=limit_hi, [63:56]=base_hi
        GDT.entries[3] = (limit & 0xFFFF)
            | ((base & 0x00FF_FFFF) << 16)
            | (0x89_u64 << 40)
            | (((limit >> 16) & 0xF) << 48)
            | (((base >> 24) & 0xFF) << 56);
        // High 8 bytes: base[63:32]
        GDT.entries[4] = (base >> 32) & 0xFFFF_FFFF;

        let ptr = GdtPtr {
            limit: (size_of::<Gdt>() - 1) as u16,
            base: core::ptr::addr_of!(GDT) as u64,
        };

        core::arch::asm!(
            "lgdt [{ptr}]",
            // Reload data segments from new GDT
            "mov ax, 0x10",
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            "xor eax, eax",
            "mov fs, ax",
            "mov gs, ax",
            // Load TSS (selector 0x18, RPL=0)
            "mov ax, 0x18",
            "ltr ax",
            ptr = in(reg) &ptr,
            out("ax") _,
            options(nostack),
        );
    }
}
