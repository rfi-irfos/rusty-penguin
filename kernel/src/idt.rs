use core::mem::size_of;
use crate::{vga, pic, port};

// CPU-pushed interrupt stack frame (low address first = IP first)
#[repr(C)]
pub struct InterruptFrame {
    pub ip:    u64,
    pub cs:    u64,
    pub flags: u64,
    pub sp:    u64,
    pub ss:    u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low:  u16,
    selector:    u16,
    ist:         u8,
    type_attr:   u8,
    offset_mid:  u16,
    offset_high: u32,
    _reserved:   u32,
}

impl IdtEntry {
    const fn missing() -> Self {
        IdtEntry { offset_low: 0, selector: 0, ist: 0, type_attr: 0,
                   offset_mid: 0, offset_high: 0, _reserved: 0 }
    }

    fn gate(handler: u64) -> Self {
        IdtEntry {
            offset_low:  (handler & 0xFFFF) as u16,
            selector:    0x08, // kernel code segment
            ist:         0,
            type_attr:   0x8E, // Present + DPL=0 + 64-bit interrupt gate
            offset_mid:  ((handler >> 16) & 0xFFFF) as u16,
            offset_high: (handler >> 32) as u32,
            _reserved:   0,
        }
    }
}

static mut IDT: [IdtEntry; 256] = [IdtEntry::missing(); 256];

#[repr(C, packed)]
struct IdtPtr { limit: u16, base: u64 }

// ── Exception handlers ────────────────────────────────────────────────────────

extern "x86-interrupt" fn exc_divide(_f: InterruptFrame) {
    vga::write_str("\nEXCEPTION: #DE divide by zero\n", vga::Color::Red);
    loop {}
}

extern "x86-interrupt" fn exc_breakpoint(_f: InterruptFrame) {
    vga::write_str("\n[BREAKPOINT]\n", vga::Color::Amber);
}

extern "x86-interrupt" fn exc_double_fault(_f: InterruptFrame, _e: u64) -> ! {
    vga::write_str("\nEXCEPTION: #DF double fault\n", vga::Color::Red);
    loop {}
}

extern "x86-interrupt" fn exc_gpf(_f: InterruptFrame, err: u64) {
    vga::write_str("\nEXCEPTION: #GP (err=0x", vga::Color::Red);
    vga::write_hex(err, vga::Color::Red);
    vga::write_str(")\n", vga::Color::Red);
    loop {}
}

extern "x86-interrupt" fn exc_page_fault(_f: InterruptFrame, err: u64) {
    let cr2: u64;
    unsafe { core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack)) };
    vga::write_str("\nEXCEPTION: #PF addr=0x", vga::Color::Red);
    vga::write_hex(cr2, vga::Color::Red);
    vga::write_str(" err=0x", vga::Color::Red);
    vga::write_hex(err, vga::Color::Red);
    vga::write_byte(b'\n', vga::Color::Red);
    loop {}
}

// ── IRQ handlers ─────────────────────────────────────────────────────────────

static mut TICKS: u64 = 0;

pub fn ticks() -> u64 {
    unsafe { TICKS }
}

// Keyboard ring buffer — filled by IRQ, drained by sys_read
const KBD_BUF_SIZE: usize = 256;
static mut KBD_BUF:  [u8; KBD_BUF_SIZE] = [0; KBD_BUF_SIZE];
static mut KBD_HEAD: usize = 0;  // next write index
static mut KBD_TAIL: usize = 0;  // next read index

pub fn kbd_get() -> Option<u8> {
    unsafe {
        if KBD_HEAD == KBD_TAIL { return None; }
        let ch = KBD_BUF[KBD_TAIL];
        KBD_TAIL = (KBD_TAIL + 1) % KBD_BUF_SIZE;
        Some(ch)
    }
}

extern "x86-interrupt" fn irq_timer(_f: InterruptFrame) {
    unsafe {
        TICKS += 1;
        pic::eoi(0);
    }
}

extern "x86-interrupt" fn irq_keyboard(_f: InterruptFrame) {
    let sc = unsafe { port::inb(0x60) };
    match sc {
        0x0E => {
            // Backspace: undo last buffered char (if any), then update display
            unsafe {
                if KBD_HEAD != KBD_TAIL {
                    KBD_HEAD = (KBD_HEAD + KBD_BUF_SIZE - 1) % KBD_BUF_SIZE;
                    vga::backspace();
                }
            }
        }
        _ => {
            if let Some(ch) = sc_to_ascii(sc) {
                vga::write_byte(ch, vga::Color::White);
                unsafe {
                    let next = (KBD_HEAD + 1) % KBD_BUF_SIZE;
                    if next != KBD_TAIL {  // drop if full
                        KBD_BUF[KBD_HEAD] = ch;
                        KBD_HEAD = next;
                    }
                }
            }
        }
    }
    unsafe { pic::eoi(1); }
}

fn sc_to_ascii(sc: u8) -> Option<u8> {
    if sc & 0x80 != 0 { return None; } // key release
    // QWERTZ layout (DE): y/z swapped vs US at positions 0x15/0x2C
    const MAP: &[u8] = b"\x00\x001234567890-=\x00\tqwertzuiop[]\n\x00asdfghjkl;'`\x00\\yxcvbnm,./\x00*\x00 ";
    MAP.get(sc as usize).copied().filter(|&b| b != 0)
}

// ── Init ─────────────────────────────────────────────────────────────────────

pub fn init() {
    unsafe {
        IDT[0]  = IdtEntry::gate(exc_divide       as *const () as u64);
        IDT[3]  = IdtEntry::gate(exc_breakpoint   as *const () as u64);
        IDT[8]  = IdtEntry::gate(exc_double_fault as *const () as u64);
        IDT[13] = IdtEntry::gate(exc_gpf          as *const () as u64);
        IDT[14] = IdtEntry::gate(exc_page_fault   as *const () as u64);
        IDT[32] = IdtEntry::gate(irq_timer        as *const () as u64);
        IDT[33] = IdtEntry::gate(irq_keyboard     as *const () as u64);

        let ptr = IdtPtr {
            limit: (size_of::<[IdtEntry; 256]>() - 1) as u16,
            base:  core::ptr::addr_of!(IDT) as u64,
        };
        core::arch::asm!("lidt [{0}]", in(reg) &ptr, options(nostack, readonly));
    }
}

pub fn enable() {
    unsafe { core::arch::asm!("sti", options(nostack)); }
}
