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

// Full register snapshot at a #GP, captured by a naked entry stub so we can see
// the faulting pointer registers (the x86-interrupt ABI hides the GPRs). Field
// order matches the push order in `exc_gpf_naked` (r15 lowest, then the CPU-
// pushed error code + iretq frame above the GPRs).
#[repr(C)]
struct GpRegs {
    r15: u64, r14: u64, r13: u64, r12: u64, r11: u64, r10: u64, r9: u64, r8: u64,
    rbp: u64, rdi: u64, rsi: u64, rdx: u64, rcx: u64, rbx: u64, rax: u64,
    err: u64, rip: u64, cs: u64, rflags: u64, rsp: u64, ss: u64,
}

#[unsafe(naked)]
extern "C" fn exc_gpf_naked() {
    core::arch::naked_asm!(
        "push rax", "push rbx", "push rcx", "push rdx",
        "push rsi", "push rdi", "push rbp",
        "push r8",  "push r9",  "push r10", "push r11",
        "push r12", "push r13", "push r14", "push r15",
        "mov rdi, rsp",          // rdi -> GpRegs (points at saved r15)
        "call {dump}",           // does not return
        "2: hlt", "jmp 2b",
        dump = sym gpf_dump,
    );
}

extern "C" fn gpf_dump(r: &GpRegs) -> ! {
    crate::serial::write_str("\n[idt] #GP err="); crate::serial::write_hex_u64(r.err);
    crate::serial::write_str(" rip=");  crate::serial::write_hex_u64(r.rip);
    crate::serial::write_str(" cs=");   crate::serial::write_hex_u64(r.cs);
    crate::serial::write_str("\n      rax="); crate::serial::write_hex_u64(r.rax);
    crate::serial::write_str(" rbx="); crate::serial::write_hex_u64(r.rbx);
    crate::serial::write_str(" rcx="); crate::serial::write_hex_u64(r.rcx);
    crate::serial::write_str(" rdx="); crate::serial::write_hex_u64(r.rdx);
    crate::serial::write_str("\n      rsi="); crate::serial::write_hex_u64(r.rsi);
    crate::serial::write_str(" rdi="); crate::serial::write_hex_u64(r.rdi);
    crate::serial::write_str(" rbp="); crate::serial::write_hex_u64(r.rbp);
    crate::serial::write_str(" rsp="); crate::serial::write_hex_u64(r.rsp);
    crate::serial::write_str("\n      r8="); crate::serial::write_hex_u64(r.r8);
    crate::serial::write_str(" r12="); crate::serial::write_hex_u64(r.r12);
    crate::serial::write_str(" r13="); crate::serial::write_hex_u64(r.r13);
    crate::serial::write_str(" r15="); crate::serial::write_hex_u64(r.r15);
    crate::serial::write_byte(b'\n');
    // Ring-3 fault during a desktop-launched Linux binary → restart the desktop.
    if r.cs == 0x23 && unsafe { crate::linux::RESTART_DESKTOP } {
        crate::serial::write_str("[idt] #GP ring-3 — restarting desktop\n");
        crate::linux::reset();
        crate::restart_desktop();
    }
    vga::write_str("\nEXCEPTION: #GP (see serial for registers)\n", vga::Color::Red);
    loop { unsafe { core::arch::asm!("hlt"); } }
}

extern "x86-interrupt" fn exc_gpf(f: InterruptFrame, err: u64) {
    // Ring-3 fault (cs=0x23) during a Linux binary run → restart desktop.
    if f.cs == 0x23 && unsafe { crate::linux::RESTART_DESKTOP } {
        crate::serial::write_str("[idt] #GP ring-3 rip=0x");
        crate::serial::write_hex_u32(f.ip as u32);
        crate::serial::write_str(" sp=0x");
        crate::serial::write_hex_u32(f.sp as u32);
        crate::serial::write_str(" err=0x");
        crate::serial::write_hex_u32(err as u32);
        crate::serial::write_str(" — restarting desktop\n");
        crate::linux::reset();
        crate::restart_desktop();
    }
    vga::write_str("\nEXCEPTION: #GP (err=0x", vga::Color::Red);
    vga::write_hex(err, vga::Color::Red);
    vga::write_str(" rip=0x", vga::Color::Red);
    vga::write_hex(f.ip, vga::Color::Red);
    vga::write_str(" cs=0x", vga::Color::Red);
    vga::write_hex(f.cs, vga::Color::Red);
    vga::write_str(")\n", vga::Color::Red);
    loop {}
}

extern "x86-interrupt" fn exc_page_fault(f: InterruptFrame, err: u64) {
    // Ring-3 fault during Linux binary run → restart desktop.
    if f.cs == 0x23 && unsafe { crate::linux::RESTART_DESKTOP } {
        crate::serial::write_str("[idt] #PF in Linux binary — restarting desktop\n");
        crate::linux::reset();
        crate::restart_desktop();
    }
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
    // Poll USB HID at the timer tick rate (~100 Hz). This drains the xHCI
    // event ring and injects keyboard/mouse events into the input ring,
    // giving USB devices the same latency as PS/2 on modern hardware.
    crate::usb::poll();
}

/// The per-tick bookkeeping the timer IRQ must do regardless of who handles it
/// (used by the scheduler's preemptive timer handler). Increments ticks, ACKs
/// the PIC, and polls USB.
pub fn timer_bookkeeping() {
    unsafe {
        TICKS += 1;
        pic::eoi(0);
    }
    crate::usb::poll();
}

/// Repoint the timer IRQ vector (IDT[32]) to `handler`. The CPU reads the IDT on
/// each interrupt, so no `lidt` reload is needed. Used to install the scheduler's
/// preemptive timer handler. `handler` is the raw entry address.
pub fn set_timer_vector(handler: u64) {
    unsafe { IDT[32] = IdtEntry::gate(handler); }
}

static mut SHIFT_DOWN: bool = false;
static mut ALTGR_DOWN: bool = false;
static mut CTRL_DOWN:  bool = false;
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
            // Arrow keys → push VT100 escape sequences (ESC [ A/B/C/D)
            0x48 | 0x50 | 0x4B | 0x4D => {
                let dir: u8 = match sc { 0x48 => b'A', 0x50 => b'B', 0x4D => b'C', _ => b'D' };
                unsafe {
                    for ch in [0x1Bu8, b'[', dir] {
                        let next = (KBD_HEAD + 1) % KBD_BUF_SIZE;
                        if next != KBD_TAIL { KBD_BUF[KBD_HEAD] = ch; KBD_HEAD = next; }
                    }
                }
                crate::input::push_key(0x1B, sc);
                crate::input::push_key(b'[', 0);
                crate::input::push_key(dir, 0);
                if crate::linux::is_linux() {
                    crate::linux::push_linux_key(0x1B);
                    crate::linux::push_linux_key(b'[');
                    crate::linux::push_linux_key(dir);
                }
            }
            // Home (ESC [ H) and End (ESC [ F)
            0x47 => {
                crate::input::push_key(0x1B, sc);
                crate::input::push_key(b'[', 0);
                crate::input::push_key(b'H', 0);
            }
            0x4F => {
                crate::input::push_key(0x1B, sc);
                crate::input::push_key(b'[', 0);
                crate::input::push_key(b'F', 0);
            }
            // Delete → ESC [ 3 ~
            0x53 => {
                crate::input::push_key(0x1B, sc);
                crate::input::push_key(b'[', 0);
                crate::input::push_key(b'3', 0);
                crate::input::push_key(b'~', 0);
            }
            _ => {}
        }
        unsafe { pic::eoi(1); }
        return;
    }

    match sc {
        0x2A | 0x36 => unsafe { SHIFT_DOWN = true; },
        0xAA | 0xB6 => unsafe { SHIFT_DOWN = false; },
        0x1D => unsafe { CTRL_DOWN = true; },
        0x9D => unsafe { CTRL_DOWN = false; },
        0x3A => unsafe { CAPS_LOCK ^= true; },
        0x0E => {
            // Backspace
            unsafe {
                let next = (KBD_HEAD + 1) % KBD_BUF_SIZE;
                if next != KBD_TAIL {
                    KBD_BUF[KBD_HEAD] = 0x08;
                    KBD_HEAD = next;
                }
            }
            crate::input::push_key(0x08, 0x0E);
            if crate::linux::is_linux() { crate::linux::push_linux_key(0x08); }
        }
        _ => {
            let shift = unsafe { SHIFT_DOWN };
            let altgr = unsafe { ALTGR_DOWN };
            let ctrl  = unsafe { CTRL_DOWN };
            let caps  = unsafe { CAPS_LOCK };
            // When Ctrl held: use bare lowercase letter then mask to control char
            let effective_shift = if ctrl { false } else { shift };
            let effective_caps  = if ctrl { false } else { caps };
            if let Some(mut ch) = sc_to_char(sc, effective_shift, altgr, effective_caps) {
                if ctrl {
                    // Map a-z / A-Z → 0x01–0x1A; other chars pass through unchanged
                    if ch >= b'a' && ch <= b'z' { ch &= 0x1F; }
                    else if ch >= b'A' && ch <= b'Z' { ch &= 0x1F; }
                }
                // No kernel-side echo — ring-3 desktop handles all display output
                unsafe {
                    let next = (KBD_HEAD + 1) % KBD_BUF_SIZE;
                    if next != KBD_TAIL {
                        KBD_BUF[KBD_HEAD] = ch;
                        KBD_HEAD = next;
                    }
                }
                crate::input::push_key(ch, sc);
                if crate::linux::is_linux() { crate::linux::push_linux_key(ch); }
            }
        }
    }
    unsafe { pic::eoi(1); }
}

// Keyboard layout: true = German QWERTZ (default), false = US English QWERTY.
// Toggled at runtime via sys_kbd_layout (#21) from the desktop tray.
static mut KBD_DE: bool = true;
pub fn set_layout_de(de: bool) { unsafe { KBD_DE = de; } }
pub fn layout_is_de() -> bool { unsafe { KBD_DE } }

fn sc_to_char(sc: u8, shift: bool, altgr: bool, caps: bool) -> Option<u8> {
    if unsafe { KBD_DE } { sc_to_char_de(sc, shift, altgr, caps) }
    else { sc_to_char_en(sc, shift, caps) }
}

// US English QWERTY layout (plain ASCII).
fn sc_to_char_en(sc: u8, shift: bool, caps: bool) -> Option<u8> {
    if sc & 0x80 != 0 { return None; }
    let up = caps ^ shift;
    Some(match sc {
        0x02 => if shift { b'!' } else { b'1' },
        0x03 => if shift { b'@' } else { b'2' },
        0x04 => if shift { b'#' } else { b'3' },
        0x05 => if shift { b'$' } else { b'4' },
        0x06 => if shift { b'%' } else { b'5' },
        0x07 => if shift { b'^' } else { b'6' },
        0x08 => if shift { b'&' } else { b'7' },
        0x09 => if shift { b'*' } else { b'8' },
        0x0A => if shift { b'(' } else { b'9' },
        0x0B => if shift { b')' } else { b'0' },
        0x0C => if shift { b'_' } else { b'-' },
        0x0D => if shift { b'+' } else { b'=' },
        0x10 => if up { b'Q' } else { b'q' },
        0x11 => if up { b'W' } else { b'w' },
        0x12 => if up { b'E' } else { b'e' },
        0x13 => if up { b'R' } else { b'r' },
        0x14 => if up { b'T' } else { b't' },
        0x15 => if up { b'Y' } else { b'y' },
        0x16 => if up { b'U' } else { b'u' },
        0x17 => if up { b'I' } else { b'i' },
        0x18 => if up { b'O' } else { b'o' },
        0x19 => if up { b'P' } else { b'p' },
        0x1A => if shift { b'{' } else { b'[' },
        0x1B => if shift { b'}' } else { b']' },
        0x1C => b'\n',
        0x1E => if up { b'A' } else { b'a' },
        0x1F => if up { b'S' } else { b's' },
        0x20 => if up { b'D' } else { b'd' },
        0x21 => if up { b'F' } else { b'f' },
        0x22 => if up { b'G' } else { b'g' },
        0x23 => if up { b'H' } else { b'h' },
        0x24 => if up { b'J' } else { b'j' },
        0x25 => if up { b'K' } else { b'k' },
        0x26 => if up { b'L' } else { b'l' },
        0x27 => if shift { b':' } else { b';' },
        0x28 => if shift { b'"' } else { b'\'' },
        0x29 => if shift { b'~' } else { b'`' },
        0x2B => if shift { b'|' } else { b'\\' },
        0x2C => if up { b'Z' } else { b'z' },
        0x2D => if up { b'X' } else { b'x' },
        0x2E => if up { b'C' } else { b'c' },
        0x2F => if up { b'V' } else { b'v' },
        0x30 => if up { b'B' } else { b'b' },
        0x31 => if up { b'N' } else { b'n' },
        0x32 => if up { b'M' } else { b'm' },
        0x33 => if shift { b'<' } else { b',' },
        0x34 => if shift { b'>' } else { b'.' },
        0x35 => if shift { b'?' } else { b'/' },
        0x39 => b' ',
        0x56 => if shift { b'>' } else { b'<' },
        _ => return None,
    })
}

// Full QWERTZ DE layout. German umlauts use CP437 encoding (VGA text mode).
// ä=0x84  Ä=0x8E  ö=0x94  Ö=0x99  ü=0x81  Ü=0x9A  ß=0xE1  °=0xF8
fn sc_to_char_de(sc: u8, shift: bool, altgr: bool, caps: bool) -> Option<u8> {
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
        IDT[13] = IdtEntry::gate(exc_gpf_naked    as *const () as u64);
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
