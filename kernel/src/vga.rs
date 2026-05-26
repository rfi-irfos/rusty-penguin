// VGA text mode driver — 80×25, direct write to physical 0xB8000
// No allocator, no runtime — pure unsafe pointer writes.

const VGA_BASE: usize = 0xB8000;
const COLS: usize = 80;
const ROWS: usize = 25;

#[allow(dead_code)]
#[derive(Clone, Copy)]
#[repr(u8)]
pub enum Color {
    Black  = 0x00,
    Blue   = 0x01,
    Green  = 0x0A,
    Cyan   = 0x0B,
    Red    = 0x0C,
    Amber  = 0x0E,  // bright yellow — matches Rusty Penguin palette
    White  = 0x0F,
    DimGray = 0x08,
}

// Global cursor position (column only — we scroll line by line)
static mut CUR_ROW: usize = 0;
static mut CUR_COL: usize = 0;

#[inline(always)]
fn vga() -> *mut u16 {
    VGA_BASE as *mut u16
}

fn cell(ch: u8, color: Color) -> u16 {
    (color as u16) << 8 | (ch as u16)
}

pub fn clear() {
    let blank = cell(b' ', Color::Black);
    for i in 0..(COLS * ROWS) {
        unsafe { vga().add(i).write_volatile(blank); }
    }
    unsafe { CUR_ROW = 0; CUR_COL = 0; }
}

fn scroll_up() {
    // Move all rows up by one
    for row in 1..ROWS {
        for col in 0..COLS {
            let src = unsafe { vga().add(row * COLS + col).read_volatile() };
            unsafe { vga().add((row - 1) * COLS + col).write_volatile(src); }
        }
    }
    // Clear last row
    let blank = cell(b' ', Color::Black);
    for col in 0..COLS {
        unsafe { vga().add((ROWS - 1) * COLS + col).write_volatile(blank); }
    }
}

pub fn write_byte(b: u8, color: Color) {
    unsafe {
        if b == b'\n' {
            CUR_COL = 0;
            CUR_ROW += 1;
        } else {
            vga().add(CUR_ROW * COLS + CUR_COL).write_volatile(cell(b, color));
            CUR_COL += 1;
            if CUR_COL >= COLS {
                CUR_COL = 0;
                CUR_ROW += 1;
            }
        }
        if CUR_ROW >= ROWS {
            scroll_up();
            CUR_ROW = ROWS - 1;
        }
    }
}

pub fn write_str(s: &str, color: Color) {
    for b in s.bytes() {
        write_byte(b, color);
    }
}

pub fn backspace() {
    unsafe {
        if CUR_COL > 0 {
            CUR_COL -= 1;
        } else if CUR_ROW > 0 {
            CUR_ROW -= 1;
            CUR_COL = COLS - 1;
        } else {
            return;
        }
        vga().add(CUR_ROW * COLS + CUR_COL).write_volatile(cell(b' ', Color::Black));
    }
}

pub fn write_hex(val: u64, color: Color) {
    let digits = b"0123456789ABCDEF";
    let mut started = false;
    for i in (0..16).rev() {
        let nibble = ((val >> (i * 4)) & 0xF) as usize;
        if nibble != 0 || started || i == 0 {
            write_byte(digits[nibble], color);
            started = true;
        }
    }
}

pub fn write_i32(n: i32) {
    if n < 0 {
        write_byte(b'-', Color::Cyan);
        write_u32((-n) as u32);
    } else {
        write_u32(n as u32);
    }
}

fn write_u32(mut n: u32) {
    if n == 0 {
        write_byte(b'0', Color::Cyan);
        return;
    }
    let mut buf = [0u8; 10];
    let mut i = 0;
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        write_byte(buf[i], Color::Cyan);
    }
}
