// Framebuffer terminal — renders a text grid to the linear framebuffer.
// Provides write_byte / write_str / write_hex / write_i32 to match vga.rs API.
// Called by vga.rs dispatch shim once FB is live.

use crate::fb;

const COLS: usize = 120;
const ROWS: usize = 40;

// Desktop-matching palette (RGB24)
pub const BLACK:   u32 = 0x000000;
pub const DARK:    u32 = 0x0B1220;  // desktop background
pub const GREEN:   u32 = 0x33FF66;
pub const AMBER:   u32 = 0xFFBB00;
pub const CYAN:    u32 = 0x00EEFF;
pub const RED:     u32 = 0xFF3333;
pub const WHITE:   u32 = 0xEEEEEE;
pub const DIMGRAY: u32 = 0x555566;
pub const BLUE:    u32 = 0x4488FF;

#[derive(Clone, Copy)]
struct Cell { ch: u8, fg: u32, bg: u32 }

impl Cell {
    const fn blank() -> Self { Cell { ch: b' ', fg: WHITE, bg: DARK } }
}

static mut GRID: [[Cell; COLS]; ROWS] = [[Cell::blank(); COLS]; ROWS];
static mut ROW: usize = 0;
static mut COL: usize = 0;

// Pixel position of the terminal origin (leave a small margin)
const MARGIN_X: u32 = 8;
const MARGIN_Y: u32 = 8;

pub fn init() {
    // Fill screen with desktop background
    fb::fill(0, 0, fb::width(), fb::height(), DARK);
    unsafe { ROW = 0; COL = 0; }
}

fn render_cell(row: usize, col: usize) {
    let cell = unsafe { GRID[row][col] };
    fb::blit_char(
        MARGIN_X + col as u32 * 8,
        MARGIN_Y + row as u32 * 9,   // 9px row height: 8px glyph + 1px gap
        cell.ch, cell.fg, cell.bg,
    );
}

fn scroll_up() {
    unsafe {
        for r in 1..ROWS {
            GRID[r - 1] = GRID[r];
        }
        GRID[ROWS - 1] = [Cell::blank(); COLS];
    }
    // Blit the entire grid (scroll is rare, ok to be slow)
    for r in 0..ROWS {
        for c in 0..COLS {
            render_cell(r, c);
        }
    }
}

pub fn write_byte_rgb(b: u8, fg: u32) {
    unsafe {
        match b {
            b'\n' => {
                COL = 0;
                ROW += 1;
                if ROW >= ROWS { scroll_up(); ROW = ROWS - 1; }
            }
            b'\r' => { COL = 0; }
            b'\t' => {
                let next = (COL + 8) & !7;
                while COL < next && COL < COLS { write_byte_rgb(b' ', fg); }
            }
            0x08 => {  // backspace
                if COL > 0 { COL -= 1; }
                GRID[ROW][COL] = Cell { ch: b' ', fg, bg: DARK };
                render_cell(ROW, COL);
            }
            _ => {
                if COL >= COLS { COL = 0; ROW += 1; }
                if ROW >= ROWS { scroll_up(); ROW = ROWS - 1; }
                GRID[ROW][COL] = Cell { ch: b, fg, bg: DARK };
                render_cell(ROW, COL);
                COL += 1;
            }
        }
    }
}

pub fn write_str_rgb(s: &str, fg: u32) {
    for b in s.bytes() { write_byte_rgb(b, fg); }
}

pub fn write_hex_rgb(val: u64, fg: u32) {
    let digits = b"0123456789ABCDEF";
    let mut started = false;
    for i in (0..16).rev() {
        let nibble = ((val >> (i * 4)) & 0xF) as usize;
        if nibble != 0 || started || i == 0 {
            write_byte_rgb(digits[nibble], fg);
            started = true;
        }
    }
}

pub fn write_u32_rgb(mut n: u32, fg: u32) {
    if n == 0 { write_byte_rgb(b'0', fg); return; }
    let mut buf = [0u8; 10];
    let mut i = 0;
    while n > 0 { buf[i] = b'0' + (n % 10) as u8; n /= 10; i += 1; }
    while i > 0 { i -= 1; write_byte_rgb(buf[i], fg); }
}

pub fn write_i32_rgb(n: i32, fg: u32) {
    if n < 0 { write_byte_rgb(b'-', fg); write_u32_rgb((-n) as u32, fg); }
    else { write_u32_rgb(n as u32, fg); }
}

/// Current cursor position for sys_fb_query responses
pub fn cursor_row() -> usize { unsafe { ROW } }
pub fn cursor_col() -> usize { unsafe { COL } }
