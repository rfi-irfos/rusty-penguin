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

fn halt() -> ! { loop { unsafe { core::arch::asm!("hlt"); } } }
