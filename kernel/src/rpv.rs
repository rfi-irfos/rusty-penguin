// rpv — "Rusty Penguin Video": a from-scratch, dependency-free video player for
// the bare-metal kernel. Plays the founding clip (Linus Torvalds, OSS EU 2024,
// "...start his own operating system in Rust...") ON the operating system it
// triggered. No web stack, no Linux, no external codec — the most meta shot.
//
// The clip is pre-decoded offline (ffmpeg + scripts/pack_rpv.py) into the .rpv
// container below and bundled in the initrd as bin/meta.rpv. This module parses
// it, decodes each frame, blits to the framebuffer (integer upscale, centered),
// and streams PCM to the HDA audio ring.
//
// .rpv format (little-endian):
//   header, 32 bytes:
//     [0..4]   magic   b"RPV1"
//     [4..8]   width   u32   (frame width in pixels, <= MAX_W)
//     [8..12]  height  u32   (frame height,          <= MAX_H)
//     [12..16] fps     u32
//     [16..20] nframes u32
//     [20..24] arate   u32   (audio sample rate, e.g. 44100)
//     [24..28] ach     u32   (audio channels, 2 = stereo)
//     [28..32] asamps  u32   (audio frames; PCM bytes = asamps*ach*2)
//   audio: asamps*ach*2 bytes, s16le interleaved
//   frames: nframes × [ u32 seglen ][ seglen bytes of segments ]
//     each frame is a delta vs the previous frame (frame 0 vs black), encoded as
//     repeated segments { u32 skip; u32 litlen; litlen×u32 pixels }:
//       advance `skip` pixels (unchanged from previous frame), then write
//       `litlen` absolute pixels (each a u32 0x00RRGGBB). Static regions cost one
//       big skip; only moving regions carry literals.

use crate::fb;

// Max frame the static decode buffer supports. 640×360 upscales 3× to exactly
// 1920×1080. One u32 per pixel = 900 KiB in .bss (kept off the 512 KiB heap).
const MAX_W: usize = 640;
const MAX_H: usize = 360;
static mut FRAME: [u32; MAX_W * MAX_H] = [0; MAX_W * MAX_H];

fn rd_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

struct Header {
    width: usize,
    height: usize,
    fps: u32,
    nframes: u32,
    arate: u32,
    ach: u32,
    asamps: u32,
}

fn parse_header(data: &[u8]) -> Option<Header> {
    if data.len() < 32 || &data[0..4] != b"RPV1" { return None; }
    let h = Header {
        width:  rd_u32(data, 4) as usize,
        height: rd_u32(data, 8) as usize,
        fps:    rd_u32(data, 12).max(1),
        nframes: rd_u32(data, 16),
        arate:  rd_u32(data, 20),
        ach:    rd_u32(data, 24),
        asamps: rd_u32(data, 28),
    };
    if h.width == 0 || h.height == 0 || h.width > MAX_W || h.height > MAX_H { return None; }
    Some(h)
}

/// Apply one frame's skip/literal segment stream into the static FRAME buffer.
/// Returns true on success. `seg` is the segment bytes for this frame.
unsafe fn apply_frame(seg: &[u8], npix: usize) -> bool {
    let mut pos = 0usize; // pixel cursor into FRAME
    let mut o = 0usize;   // byte cursor into seg
    while o + 8 <= seg.len() {
        let skip   = rd_u32(seg, o) as usize; o += 4;
        let litlen = rd_u32(seg, o) as usize; o += 4;
        pos += skip;
        if pos + litlen > npix || o + litlen * 4 > seg.len() { return false; }
        for i in 0..litlen {
            FRAME[pos + i] = rd_u32(seg, o + i * 4);
        }
        o += litlen * 4;
        pos += litlen;
    }
    true
}

/// Blit FRAME (w×h) to the framebuffer, nearest-neighbour upscaled by `scale`
/// and centred. Writes directly through fb::pixel.
fn blit(w: usize, h: usize, scale: usize) {
    let fw = fb::width() as usize;
    let fh = fb::height() as usize;
    let ox = (fw.saturating_sub(w * scale)) / 2;
    let oy = (fh.saturating_sub(h * scale)) / 2;
    unsafe {
        for y in 0..h {
            for x in 0..w {
                let rgb = FRAME[y * w + x];
                let px = ox + x * scale;
                let py = oy + y * scale;
                for dy in 0..scale {
                    for dx in 0..scale {
                        fb::pixel((px + dx) as u32, (py + dy) as u32, rgb);
                    }
                }
            }
        }
    }
}

// ── Windowed video service — the desktop Media app drives this via syscalls ───
// The kernel owns the decoder + the (large) .rpv in ramfs + the HDA ring, so the
// ring-3 desktop never loads 58 MiB into its heap: it draws the window chrome,
// asks the kernel to step a frame (decoding + streaming a frame's worth of audio),
// then asks it to scale the current frame straight into the desktop's backbuffer.
struct VideoState {
    data: *const u8, len: usize,
    w: usize, h: usize, nframes: usize, fps: usize,
    frames_start: usize, cur_off: usize, cur_frame: usize, open: bool,
    // Audio (PCM blob between header and frames). Streamed one frame at a time.
    pcm_off: usize, pcm_len: usize, apos: usize, ring: usize,
    arate: usize, have_audio: bool,
}
static mut VS: VideoState = VideoState {
    data: core::ptr::null(), len: 0, w: 0, h: 0, nframes: 0, fps: 0,
    frames_start: 0, cur_off: 0, cur_frame: 0, open: false,
    pcm_off: 0, pcm_len: 0, apos: 0, ring: 0, arate: 0, have_audio: false,
};

/// Open bin/meta.rpv for windowed playback and prime the audio ring. Returns
/// packed dims (w<<48)|(h<<32)|(fps<<24)|(nframes & 0xFFFFFF), or 0 if unavailable.
pub fn service_open() -> u64 {
    let data = match crate::ramfs::find(b"bin/meta.rpv") { Some(d) => d, None => return 0 };
    let h = match parse_header(data) { Some(h) => h, None => return 0 };
    let pcm_off = 32usize;
    let pcm_bytes = (h.asamps as usize) * (h.ach as usize) * 2;
    let pcm_len = if pcm_off + pcm_bytes <= data.len() { pcm_bytes } else { 0 };
    let frames_start = pcm_off + pcm_bytes;
    let have_audio = pcm_len > 0 && crate::hda::is_ready() && h.ach == 2 && h.arate == 44100;
    unsafe {
        VS = VideoState { data: data.as_ptr(), len: data.len(), w: h.width, h: h.height,
            nframes: h.nframes as usize, fps: h.fps as usize, frames_start,
            cur_off: frames_start, cur_frame: 0, open: true,
            pcm_off, pcm_len, apos: 0, ring: 0, arate: h.arate as usize, have_audio };
        for p in FRAME.iter_mut() { *p = 0; }
        if have_audio {
            let pcm = core::slice::from_raw_parts(data.as_ptr().add(pcm_off), pcm_len);
            let ring_sz = crate::hda::audio_ring_bytes();
            let prime = pcm.len().min(ring_sz);
            crate::hda::audio_write(&pcm[..prime], 0);
            VS.apos = prime; VS.ring = prime % ring_sz.max(1);
        }
    }
    ((h.width as u64) << 48) | ((h.height as u64) << 32) | ((h.fps as u64) << 24) | ((h.nframes as u64) & 0xFF_FFFF)
}

/// Step to the next frame: decode it into FRAME and stream a frame's worth of
/// audio just ahead of the play cursor, looping at the end. Returns
/// (level << 32) | frame_index, where `level` is a 0..255 RMS of the audio
/// chunk just queued — the desktop visualizer pulses to it.
pub fn service_advance() -> u64 {
    unsafe {
        if !VS.open { return 0; }
        let data = core::slice::from_raw_parts(VS.data, VS.len);
        if VS.cur_frame >= VS.nframes {
            VS.cur_off = VS.frames_start; VS.cur_frame = 0;
            VS.apos = 0; VS.ring = 0;
            for p in FRAME.iter_mut() { *p = 0; }
        }
        let o = VS.cur_off;
        if o + 4 > data.len() { return VS.cur_frame as u64; }
        let seglen = rd_u32(data, o) as usize;
        if o + 4 + seglen > data.len() { return VS.cur_frame as u64; }
        apply_frame(&data[o + 4..o + 4 + seglen], VS.w * VS.h);
        VS.cur_off = o + 4 + seglen;
        VS.cur_frame += 1;

        let mut level: u64 = 0;
        if VS.have_audio && VS.fps > 0 {
            let pcm = core::slice::from_raw_parts(VS.data.add(VS.pcm_off), VS.pcm_len);
            let bpf = (VS.arate * 4) / VS.fps; // stereo s16 = 4 bytes/sample-frame
            if bpf > 0 && VS.apos < pcm.len() {
                let end = (VS.apos + bpf).min(pcm.len());
                let chunk = &pcm[VS.apos..end];
                // Rough peak amplitude over the chunk → 0..255 visualizer drive.
                let mut peak: i32 = 0;
                let mut i = 0;
                while i + 1 < chunk.len() {
                    let s = i16::from_le_bytes([chunk[i], chunk[i + 1]]) as i32;
                    let a = s.abs();
                    if a > peak { peak = a; }
                    i += 64; // sparse sample — peak detection only
                }
                level = ((peak * 255) / 32768) as u64;
                let ring_sz = crate::hda::audio_ring_bytes().max(1);
                crate::hda::audio_write(chunk, VS.ring);
                VS.ring = (VS.ring + chunk.len()) % ring_sz;
                VS.apos = end;
            }
        }
        (level << 32) | (VS.cur_frame as u64 & 0xFFFF_FFFF)
    }
}

/// Scale the current FRAME into the desktop's backbuffer (user memory), centred
/// and aspect-preserved (letterbox) inside the (dw×dh) content rect at (dx,dy).
/// `back_base` is the desktop's backbuffer pointer; it shares the hardware fb's
/// pitch/bpp, so we reuse the kernel's fb geometry to address it.
pub fn service_blit(back_base: u64, dx: usize, dy: usize, dw: usize, dh: usize) {
    let (w, h) = unsafe { if !VS.open { return; } (VS.w, VS.h) };
    if w == 0 || h == 0 || dw == 0 || dh == 0 || back_base == 0 { return; }
    let pitch = fb::pitch() as usize;
    let bpp   = (fb::bpp() / 8) as usize;
    let fw    = fb::width() as usize;
    let fh    = fb::height() as usize;
    if bpp < 3 || pitch == 0 { return; }
    // Fit (×1024 fixed point to avoid divide-by-w underflow on small windows).
    let s = (dw * 1024 / w).min(dh * 1024 / h);
    let outw = w * s / 1024; let outh = h * s / 1024;
    if outw == 0 || outh == 0 { return; }
    let ox = dx + (dw - outw) / 2; let oy = dy + (dh - outh) / 2;
    let base = back_base as *mut u8;
    unsafe {
        for yy in 0..outh {
            let py = oy + yy;
            if py >= fh { break; }
            let sy = yy * h / outh;
            let row = py * pitch;
            for xx in 0..outw {
                let px = ox + xx;
                if px >= fw { continue; }
                let sx = xx * w / outw;
                let rgb = FRAME[sy * w + sx];
                let off = row + px * bpp;
                *base.add(off)     = (rgb & 0xFF) as u8;          // B
                *base.add(off + 1) = ((rgb >> 8) & 0xFF) as u8;   // G
                *base.add(off + 2) = ((rgb >> 16) & 0xFF) as u8;  // R
                if bpp == 4 { *base.add(off + 3) = 0xFF; }
            }
        }
    }
}

/// Busy-wait roughly `ms` milliseconds using the PIT tick counter (paces fps).
fn delay_ms(ms: u32) {
    let start = crate::idt::ticks();
    // PIT runs at ~100 Hz (pic::pit_init) → ~10 ms per tick. idt::ticks() is a
    // volatile read, so this busy-wait actually observes the IRQ's updates.
    let want = (ms / 10).max(1) as u64;
    let mut spins: u64 = 0;
    while crate::idt::ticks().wrapping_sub(start) < want {
        spins += 1;
        if spins > 200_000_000 { break; } // safety net so we never hang
        unsafe { core::arch::asm!("pause", options(nomem, nostack)); }
    }
}

/// Play bin/meta.rpv from the initrd. Boot flag `metavideo`. Never returns.
pub fn play_from_initrd() -> ! {
    use crate::serial::{write_str, write_hex_u64};
    write_str("\n[rpv] === meta video player ===\n");
    let data = match crate::ramfs::find(b"bin/meta.rpv") {
        Some(d) => d,
        None => { write_str("[rpv] bin/meta.rpv not in initrd — nothing to play\n"); halt(); }
    };
    let h = match parse_header(data) {
        Some(h) => h,
        None => { write_str("[rpv] bad/unsupported .rpv header\n"); halt(); }
    };
    write_str("[rpv] "); write_hex_u64(h.width as u64); write_str(" x ");
    write_hex_u64(h.height as u64); write_str("  frames="); write_hex_u64(h.nframes as u64);
    write_str("  fps="); write_hex_u64(h.fps as u64); write_str("\n");

    let npix = h.width * h.height;
    let fw = fb::width() as usize;
    let fh = fb::height() as usize;
    let scale = (fw / h.width).min(fh / h.height).max(1);

    // Audio: PCM blob follows the 32-byte header. Prime the HDA ring with the
    // first chunk and stream the rest in step with the video frames.
    let pcm_off = 32usize;
    let pcm_bytes = (h.asamps as usize) * (h.ach as usize) * 2;
    let pcm: &[u8] = if pcm_off + pcm_bytes <= data.len() {
        &data[pcm_off..pcm_off + pcm_bytes]
    } else { &[] };
    let mut apos = 0usize;        // byte cursor into pcm
    let mut ring = 0usize;        // byte offset into the HDA ring
    let ring_sz = crate::hda::audio_ring_bytes();
    let bytes_per_frame_aud = if h.fps > 0 { (h.arate as usize * 4) / h.fps as usize } else { 0 };
    let have_audio = !pcm.is_empty() && crate::hda::is_ready() && h.ach == 2 && h.arate == 44100;
    if have_audio {
        let prime = pcm.len().min(ring_sz);
        crate::hda::audio_write(&pcm[..prime], 0);
        apos = prime; ring = prime % ring_sz.max(1);
    }

    // Frame stream starts after header + audio.
    let mut o = pcm_off + pcm_bytes;
    for f in 0..h.nframes {
        if o + 4 > data.len() { write_str("[rpv] truncated frame table\n"); break; }
        let seglen = rd_u32(data, o) as usize; o += 4;
        if o + seglen > data.len() { write_str("[rpv] truncated frame data\n"); break; }
        if unsafe { !apply_frame(&data[o..o + seglen], npix) } {
            write_str("[rpv] frame decode error at "); write_hex_u64(f as u64); write_str("\n"); break;
        }
        o += seglen;
        blit(h.width, h.height, scale);

        // Stream a frame's worth of audio just ahead of the play cursor.
        if have_audio && bytes_per_frame_aud > 0 && apos < pcm.len() {
            let end = (apos + bytes_per_frame_aud).min(pcm.len());
            let chunk = &pcm[apos..end];
            crate::hda::audio_write(chunk, ring);
            ring = (ring + chunk.len()) % ring_sz.max(1);
            apos = end;
        }

        // Progress trace ~once per second of playback.
        if f % h.fps == 0 { write_str("[rpv] frame "); write_hex_u64(f as u64); write_str("\n"); }
        delay_ms(1000 / h.fps);
    }
    write_str("[rpv] === playback complete ===\n");
    // Leave the last frame on screen.
    halt();
}

/// Self-test for the windowed service (boot flag `videowin`): exercise the exact
/// path the desktop Media app drives — service_open → service_advance (decode +
/// audio) → service_blit (scale into a window-sized rect) — but draw straight onto
/// the real framebuffer with a faux window chrome so it can be screendumped
/// headlessly. Proves the syscall plumbing end-to-end without the GUI/mouse.
pub fn selftest_window() -> ! {
    use crate::serial::{write_str, write_hex_u64};
    write_str("\n[rpv] === windowed service self-test (videowin) ===\n");
    let packed = service_open();
    if packed == 0 { write_str("[rpv] service_open failed — bin/meta.rpv missing?\n"); halt(); }
    let w = (packed >> 48) & 0xFFFF;
    let h = (packed >> 32) & 0xFFFF;
    let fps = (packed >> 24) & 0xFF;
    let nf = packed & 0xFF_FFFF;
    write_str("[rpv] open ok w="); write_hex_u64(w);
    write_str(" h="); write_hex_u64(h);
    write_str(" fps="); write_hex_u64(fps);
    write_str(" nframes="); write_hex_u64(nf); write_str("\n");

    // Centred window rect (mimics the desktop Media window: 560x386 incl. chrome).
    let fw = fb::width() as usize;
    let fh = fb::height() as usize;
    let win_w = 560.min(fw);
    let win_h = 386.min(fh);
    let wx = (fw - win_w) / 2;
    let wy = (fh - win_h) / 2;
    let bar = 26;
    let content_h = win_h - bar;
    let base = fb::base() as u64; // blit straight onto the real fb (no backbuffer)

    // Faux chrome: dark titlebar + black content backing.
    fb::fill(wx as u32, wy as u32, win_w as u32, win_h as u32, 0x14171A);
    fb::fill(wx as u32, (wy + bar) as u32, win_w as u32, content_h as u32, 0x000000);

    let mut total: u64 = 0;
    for _ in 0..nf {
        let r = service_advance();
        let level = (r >> 32) & 0xFF;
        total = total.wrapping_add(level);
        service_blit(base, wx, wy + bar, win_w, content_h);
        delay_ms(if fps > 0 { (1000 / fps) as u32 } else { 33 });
    }
    write_str("[rpv] window self-test complete, level-sum="); write_hex_u64(total); write_str("\n");
    halt();
}

fn halt() -> ! { loop { unsafe { core::arch::asm!("hlt"); } } }
