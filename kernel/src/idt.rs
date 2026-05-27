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

extern "x86-interrupt" fn exc_page_fault(f: InterruptFrame, err: u64) {
    let cr2: u64;
    unsafe { core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack)) };
    vga::write_str("\nEXCEPTION: #PF addr=0x", vga::Color::Red);
    vga::write_hex(cr2, vga::Color::Red);
    vga::write_str(" err=0x", vga::Color::Red);
    vga::write_hex(err, vga::Color::Red);
    vga::write_str(" rip=0x", vga::Color::Red);
    vga::write_hex(f.ip, vga::Color::Red);
    vga::write_str(" rsp=0x", vga::Color::Red);
    vga::write_hex(f.sp, vga::Color::Red);
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

static mut SHIFT_DOWN: bool = false;
static mut ALTGR_DOWN: bool = false;
static mut CAPS_LOCK:  bool = false;
static mut E0_PREFIX:  bool = false;

extern "x86-interrupt" fn irq_mouse(_f: InterruptFrame) {
    crate::ps2mouse::handle_irq();
    unsafe { pic::eoi(12); }
}

extern "x86-interrupt" fn irq_keyboard(_f: InterruptFrame) {
    let sc = unsafe { port::inb(0x60) };

    // E0 prefix = extended key; next byte is the real scancode
    if sc == 0xE0 {
        unsafe { E0_PREFIX = true; }
        unsafe { pic::eoi(1); }
        return;
    }

    let extended = unsafe { E0_PREFIX };
    unsafe { E0_PREFIX = false; }

    if extended {
        match sc {
            0x38 => unsafe { ALTGR_DOWN = true; },
            0xB8 => unsafe { ALTGR_DOWN = false; },
            _ => {}
        }
        unsafe { pic::eoi(1); }
        return;
    }

    match sc {
        0x2A | 0x36 => unsafe { SHIFT_DOWN = true; },
        0xAA | 0xB6 => unsafe { SHIFT_DOWN = false; },
        0x3A => unsafe { CAPS_LOCK ^= true; },
        0x0E => {
            // Backspace: deliver 0x08 to ring-3 via both queues; ring-3 handles echo
            unsafe {
                let next = (KBD_HEAD + 1) % KBD_BUF_SIZE;
                if next != KBD_TAIL {
                    KBD_BUF[KBD_HEAD] = 0x08;
                    KBD_HEAD = next;
                }
            }
            crate::input::push_key(0x08, 0x0E);
        }
        _ => {
            let shift = unsafe { SHIFT_DOWN };
            let altgr = unsafe { ALTGR_DOWN };
            let caps  = unsafe { CAPS_LOCK };
            if let Some(ch) = sc_to_char(sc, shift, altgr, caps) {
                // No kernel-side echo — ring-3 desktop handles all display output
                unsafe {
                    let next = (KBD_HEAD + 1) % KBD_BUF_SIZE;
                    if next != KBD_TAIL {
                        KBD_BUF[KBD_HEAD] = ch;
                        KBD_HEAD = next;
                    }
                }
                crate::input::push_key(ch, sc);
            }
        }
    }
    unsafe { pic::eoi(1); }
}

// Full QWERTZ DE layout. German umlauts use CP437 encoding (VGA text mode).
// ä=0x84  Ä=0x8E  ö=0x94  Ö=0x99  ü=0x81  Ü=0x9A  ß=0xE1  °=0xF8
fn sc_to_char(sc: u8, shift: bool, altgr: bool, caps: bool) -> Option<u8> {
    if sc & 0x80 != 0 { return None; }
    let up = caps ^ shift;  // true = uppercase letter
    Some(match sc {
        // Number row
        0x02 => if shift { b'!' } else { b'1' },
        0x03 => if shift { b'"' } else { b'2' },
        0x04 => if shift { 0x15 } else { b'3' },   // §=CP437 0x15
        0x05 => if shift { b'$' } else { b'4' },
        0x06 => if shift { b'%' } else { b'5' },
        0x07 => if shift { b'&' } else { b'6' },
        0x08 => if altgr { b'{' } else if shift { b'/' } else { b'7' },
        0x09 => if altgr { b'[' } else if shift { b'(' } else { b'8' },
        0x0A => if altgr { b']' } else if shift { b')' } else { b'9' },
        0x0B => if altgr { b'}' } else if shift { b'=' } else { b'0' },
        0x0C => if altgr { b'\\' } else if shift { b'?' } else { 0xE1 }, // ß
        0x0D => if shift { b'`' } else { b'\'' },
        // Top row (QWERTZ)
        0x10 => if altgr { b'@' } else if up { b'Q' } else { b'q' },
        0x11 => if up { b'W' } else { b'w' },
        0x12 => if up { b'E' } else { b'e' },
        0x13 => if up { b'R' } else { b'r' },
        0x14 => if up { b'T' } else { b't' },
        0x15 => if up { b'Z' } else { b'z' },
        0x16 => if up { b'U' } else { b'u' },
        0x17 => if up { b'I' } else { b'i' },
        0x18 => if up { b'O' } else { b'o' },
        0x19 => if up { b'P' } else { b'p' },
        0x1A => if up { 0x9A } else { 0x81 },        // Ü / ü
        0x1B => if altgr { b'~' } else if shift { b'*' } else { b'+' },
        0x1C => b'\n',
        // Home row
        0x1E => if up { b'A' } else { b'a' },
        0x1F => if up { b'S' } else { b's' },
        0x20 => if up { b'D' } else { b'd' },
        0x21 => if up { b'F' } else { b'f' },
        0x22 => if up { b'G' } else { b'g' },
        0x23 => if up { b'H' } else { b'h' },
        0x24 => if up { b'J' } else { b'j' },
        0x25 => if up { b'K' } else { b'k' },
        0x26 => if up { b'L' } else { b'l' },
        0x27 => if up { 0x99 } else { 0x94 },        // Ö / ö
        0x28 => if up { 0x8E } else { 0x84 },        // Ä / ä
        0x29 => if shift { 0xF8 } else { b'^' },     // ° / ^
        0x2B => if shift { b'\'' } else { b'#' },
        // Bottom row
        0x2C => if up { b'Y' } else { b'y' },
        0x2D => if up { b'X' } else { b'x' },
        0x2E => if up { b'C' } else { b'c' },
        0x2F => if up { b'V' } else { b'v' },
        0x30 => if up { b'B' } else { b'b' },
        0x31 => if up { b'N' } else { b'n' },
        0x32 => if up { b'M' } else { b'm' },
        0x33 => if shift { b';' } else { b',' },
        0x34 => if shift { b':' } else { b'.' },
        0x35 => if shift { b'_' } else { b'-' },
        0x39 => b' ',
        0x56 => if shift { b'>' } else { b'<' },  // extra DE key (between LShift and Y)
        _ => return None,
    })
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
        IDT[44] = IdtEntry::gate(irq_mouse        as *const () as u64);

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
