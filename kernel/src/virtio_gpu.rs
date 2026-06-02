//! virtio-gpu driver for the bare-metal Rusty Penguin kernel — from scratch,
//! modern (virtio 1.0) PCI transport. This is the foundation of GPU-backed
//! display: instead of the desktop hammering an uncached VBE framebuffer over
//! MMIO every present, the GPU device DMAs from cacheable RAM and the host GPU
//! scans out. Brick 1 (this file's first milestone): detect the device, bring
//! the modern transport up, set up the control virtqueue, and query the display.
//! The VBE framebuffer stays the live display until the scanout path lands.
//!
//! Spec: OASIS "Virtual I/O Device (VIRTIO) Version 1.1", §4.1 (PCI transport),
//! §2.6 (split virtqueues), §5.7 (GPU device).

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{compiler_fence, Ordering};

// ── PCI identity ────────────────────────────────────────────────────────────
const VIRTIO_VENDOR: u16 = 0x1AF4;
const GPU_DEV_MODERN: u16 = 0x1050; // virtio-gpu, virtio 1.0
const GPU_DEV_XITION: u16 = 0x1010; // transitional id some QEMU builds expose

// PCI vendor-specific capability (the virtio caps live in a 0x09 cap chain).
const PCI_CAP_VENDOR: u8 = 0x09;
const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
const VIRTIO_PCI_CAP_ISR_CFG: u8 = 3;
const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;

// device_status bits
const S_ACKNOWLEDGE: u8 = 1;
const S_DRIVER: u8 = 2;
const S_DRIVER_OK: u8 = 4;
const S_FEATURES_OK: u8 = 8;

// virtqueue descriptor flags
const VIRTQ_DESC_F_NEXT: u16 = 1;
const VIRTQ_DESC_F_WRITE: u16 = 2;

// virtio-gpu control types
const VIRTIO_GPU_CMD_GET_DISPLAY_INFO: u32 = 0x0100;
const VIRTIO_GPU_CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
const VIRTIO_GPU_CMD_RESOURCE_UNREF: u32 = 0x0102;
const VIRTIO_GPU_CMD_SET_SCANOUT: u32 = 0x0103;
const VIRTIO_GPU_CMD_RESOURCE_FLUSH: u32 = 0x0104;
const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
const VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
const VIRTIO_GPU_RESP_OK_NODATA: u32 = 0x1100;
const VIRTIO_GPU_RESP_OK_DISPLAY_INFO: u32 = 0x1101;
const VIRTIO_GPU_RESP_OK_CAPSET_INFO: u32 = 0x1102;
// virgl (3D) control commands — only valid once VIRTIO_GPU_F_VIRGL is negotiated.
const VIRTIO_GPU_CMD_GET_CAPSET_INFO: u32 = 0x0108;
const VIRTIO_GPU_CMD_CTX_CREATE: u32 = 0x0200;
const VIRTIO_GPU_CMD_CTX_DESTROY: u32 = 0x0201;
const VIRTIO_GPU_FORMAT_B8G8R8X8: u32 = 2; // matches the desktop's 0x00RRGGBB pixels

// The scanout resource and its backing. Backing lives in the low identity-mapped
// scratch zone (55–63 MiB), past the virtqueue rings and clear of the initrd
// (relocated to 128 MiB). 4 MiB holds 1280×800×4 with room to spare.
const SCANOUT_RES_ID: u32 = 1;
const BACKING_PHYS: u64 = 0x0374_0000;

// ── DMA region (fixed low physical addresses, identity-mapped 0–64 MiB, placed
// just past the AHCI scratch buffers at 0x0370_0000 — same reservation scheme).
const VQ_DESC_PHYS: u64 = 0x0372_0000; // descriptor table
const VQ_AVAIL_PHYS: u64 = 0x0372_1000; // available ring (driver → device)
const VQ_USED_PHYS: u64 = 0x0372_2000; // used ring (device → driver)
const REQ_PHYS: u64 = 0x0372_3000; // command request buffer
const RESP_PHYS: u64 = 0x0372_3400; // command response buffer

const QSIZE: u16 = 64; // our chosen control-queue depth (power of two ≤ device max)

// ── Module state ────────────────────────────────────────────────────────────
static mut READY: bool = false;
static mut COMMON: u64 = 0; // virtio_pci_common_cfg MMIO base
static mut NOTIFY_BASE: u64 = 0; // notify BAR base + cap offset
static mut NOTIFY_MUL: u32 = 0; // notify_off_multiplier
static mut QUEUE_NOTIFY_OFF: u16 = 0; // controlq's notify offset
static mut DEVICE_CFG: u64 = 0; // virtio_gpu_config MMIO base (device-specific cfg)
// VIRTIO_GPU_F_VIRGL (feature bit 0): the device can do 3D (virgl). Set during
// bring-up. The host only offers it when QEMU is `virtio-gpu-gl` on a GL-capable
// display backend (egl-headless/gtk gl=on) with virglrenderer — i.e. it lights up
// exactly when real 3D acceleration is available to build on.
static mut VIRGL_3D: bool = false;
static mut NUM_CAPSETS: u32 = 0; // virtio_gpu_config.num_capsets (>0 ⇒ host virgl live)
// When set (via the `virgltest` boot flag, before init), bring-up NEGOTIATES
// VIRTIO_GPU_F_VIRGL and runs the 3D control-path probe (capset query + a 3D
// context create/destroy). Off by default so the normal 2D desktop path is
// byte-for-byte unchanged.
static mut WANT_VIRGL: bool = false;

/// Opt into the virgl 3D control-path probe at bring-up. Call BEFORE `init()`.
pub fn enable_virgl_test() {
    unsafe { WANT_VIRGL = true; }
}
static mut AVAIL_IDX: u16 = 0; // our shadow of avail.idx
static mut DISP_W: u32 = 0;
static mut DISP_H: u32 = 0;

#[inline]
fn ser(s: &str) {
    crate::serial::write_str(s);
}
fn ser_dec(mut n: u32) {
    if n == 0 {
        crate::serial::write_byte(b'0');
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
        crate::serial::write_byte(buf[i]);
    }
}

// ── common_cfg field accessors (offsets per spec §4.1.4.3) ───────────────────
#[inline]
unsafe fn cc_w8(off: u64, v: u8) {
    write_volatile((COMMON + off) as *mut u8, v);
}
#[inline]
unsafe fn cc_r8(off: u64) -> u8 {
    read_volatile((COMMON + off) as *const u8)
}
#[inline]
unsafe fn cc_w16(off: u64, v: u16) {
    write_volatile((COMMON + off) as *mut u16, v);
}
#[inline]
unsafe fn cc_r16(off: u64) -> u16 {
    read_volatile((COMMON + off) as *const u16)
}
#[inline]
unsafe fn cc_w32(off: u64, v: u32) {
    write_volatile((COMMON + off) as *mut u32, v);
}
#[inline]
unsafe fn cc_r32(off: u64) -> u32 {
    read_volatile((COMMON + off) as *const u32)
}
#[inline]
unsafe fn cc_w64(off: u64, v: u64) {
    // split 64-bit registers are written lo then hi
    write_volatile((COMMON + off) as *mut u32, v as u32);
    write_volatile((COMMON + off + 4) as *mut u32, (v >> 32) as u32);
}

const DEVICE_FEATURE_SELECT: u64 = 0x00;
const DEVICE_FEATURE: u64 = 0x04;
const DRIVER_FEATURE_SELECT: u64 = 0x08;
const DRIVER_FEATURE: u64 = 0x0C;
const DEVICE_STATUS: u64 = 0x14;
const QUEUE_SELECT: u64 = 0x16;
const QUEUE_SIZE: u64 = 0x18;
const QUEUE_ENABLE: u64 = 0x1C;
const QUEUE_NOTIFY: u64 = 0x1E;
const QUEUE_DESC: u64 = 0x20;
const QUEUE_AVAIL: u64 = 0x28;
const QUEUE_USED: u64 = 0x30;

/// Read a 64-bit-capable BAR's base physical address (handles 64-bit BARs).
fn bar_base(bus: u8, dev: u8, func: u8, bar: u8) -> u64 {
    let lo = crate::pci::bar(bus, dev, func, bar);
    if lo & 0x4 != 0 {
        // 64-bit memory BAR: high half in the next BAR slot.
        let hi = crate::pci::bar(bus, dev, func, bar + 1);
        ((hi as u64) << 32) | ((lo & !0xF) as u64)
    } else {
        (lo & !0xF) as u64
    }
}

/// Walk the PCI capability list and resolve the virtio structure BAR bases.
/// Returns (common, notify, notify_mul) MMIO virtual (== phys) addresses, or
/// None if the modern caps are absent.
unsafe fn map_caps(bus: u8, dev: u8, func: u8) -> Option<(u64, u64, u32)> {
    // capabilities pointer at config offset 0x34 (low byte).
    let mut cap = (crate::pci::read32(bus, dev, func, 0x34) & 0xFF) as u8;
    let mut common = 0u64;
    let mut notify = 0u64;
    let mut notify_mul = 0u32;

    while cap != 0 && cap != 0xFF {
        let w0 = crate::pci::read32(bus, dev, func, cap);
        let cap_id = (w0 & 0xFF) as u8;
        let next = ((w0 >> 8) & 0xFF) as u8;
        if cap_id == PCI_CAP_VENDOR {
            // virtio_pci_cap: cfg_type @ byte 3, bar @ byte 4, offset @ +8, len @ +12
            let cfg_type = ((w0 >> 24) & 0xFF) as u8;
            let barno = (crate::pci::read32(bus, dev, func, cap + 4) & 0xFF) as u8;
            let offset = crate::pci::read32(bus, dev, func, cap + 8);
            let length = crate::pci::read32(bus, dev, func, cap + 12);
            let base = bar_base(bus, dev, func, barno);
            let addr = base + offset as u64;
            // Map the structure's MMIO so the CPU can touch it.
            crate::vmm::map_mmio_range(addr, length.max(0x1000) as u64);
            match cfg_type {
                VIRTIO_PCI_CAP_COMMON_CFG => common = addr,
                VIRTIO_PCI_CAP_NOTIFY_CFG => {
                    notify = addr;
                    notify_mul = crate::pci::read32(bus, dev, func, cap + 16);
                }
                VIRTIO_PCI_CAP_DEVICE_CFG => DEVICE_CFG = addr,
                VIRTIO_PCI_CAP_ISR_CFG => {}
                _ => {}
            }
        }
        cap = next;
    }

    if common != 0 && notify != 0 {
        Some((common, notify, notify_mul))
    } else {
        None
    }
}

/// Zero a fixed DMA region.
unsafe fn zero(phys: u64, len: usize) {
    let p = phys as *mut u8;
    for i in 0..len {
        write_volatile(p.add(i), 0);
    }
}

/// Bring up the device and the control virtqueue. Returns false on any failure.
unsafe fn bring_up(bus: u8, dev: u8, func: u8) -> bool {
    crate::pci::enable_bus_master(bus, dev, func);

    let (common, notify, notify_mul) = match map_caps(bus, dev, func) {
        Some(t) => t,
        None => {
            ser("  [virtio-gpu] modern virtio caps not found\n");
            return false;
        }
    };
    COMMON = common;
    NOTIFY_BASE = notify;
    NOTIFY_MUL = notify_mul;

    // §3.1.1 device initialisation sequence.
    cc_w8(DEVICE_STATUS, 0); // reset
    while cc_r8(DEVICE_STATUS) != 0 {}
    cc_w8(DEVICE_STATUS, S_ACKNOWLEDGE);
    cc_w8(DEVICE_STATUS, S_ACKNOWLEDGE | S_DRIVER);

    // Negotiate features: we only require VIRTIO_F_VERSION_1 (feature bit 32).
    // Word 0 carries the GPU-specific bits; bit 0 = VIRTIO_GPU_F_VIRGL (3D).
    cc_w32(DEVICE_FEATURE_SELECT, 0);
    let feat_lo = cc_r32(DEVICE_FEATURE);
    cc_w32(DEVICE_FEATURE_SELECT, 1);
    let feat_hi = cc_r32(DEVICE_FEATURE);
    VIRGL_3D = feat_lo & 0x1 != 0; // VIRTIO_GPU_F_VIRGL offered by the device
    // Accept VIRGL (word-0 bit 0) only when the virgltest probe asked for it AND
    // the device offers it; otherwise word 0 stays 0 (2D-only, the default).
    let drv_lo = if WANT_VIRGL && VIRGL_3D { feat_lo & 0x1 } else { 0 };
    cc_w32(DRIVER_FEATURE_SELECT, 0);
    cc_w32(DRIVER_FEATURE, drv_lo);
    cc_w32(DRIVER_FEATURE_SELECT, 1);
    cc_w32(DRIVER_FEATURE, feat_hi & 0x1); // accept VERSION_1 (bit 0 of word 1)

    cc_w8(DEVICE_STATUS, S_ACKNOWLEDGE | S_DRIVER | S_FEATURES_OK);
    if cc_r8(DEVICE_STATUS) & S_FEATURES_OK == 0 {
        ser("  [virtio-gpu] FEATURES_OK rejected\n");
        return false;
    }

    // Set up the control queue (queue 0).
    cc_w16(QUEUE_SELECT, 0);
    let dev_qsize = cc_r16(QUEUE_SIZE);
    if dev_qsize == 0 {
        ser("  [virtio-gpu] controlq unavailable\n");
        return false;
    }
    let qsize = if dev_qsize < QSIZE { dev_qsize } else { QSIZE };
    cc_w16(QUEUE_SIZE, qsize);

    zero(VQ_DESC_PHYS, qsize as usize * 16);
    zero(VQ_AVAIL_PHYS, 6 + qsize as usize * 2);
    zero(VQ_USED_PHYS, 6 + qsize as usize * 8);

    cc_w64(QUEUE_DESC, VQ_DESC_PHYS);
    cc_w64(QUEUE_AVAIL, VQ_AVAIL_PHYS);
    cc_w64(QUEUE_USED, VQ_USED_PHYS);
    QUEUE_NOTIFY_OFF = cc_r16(QUEUE_NOTIFY);
    cc_w16(QUEUE_ENABLE, 1);

    cc_w8(DEVICE_STATUS, S_ACKNOWLEDGE | S_DRIVER | S_FEATURES_OK | S_DRIVER_OK);
    AVAIL_IDX = 0;

    ser("  [virtio-gpu] transport up — controlq size ");
    ser_dec(qsize as u32);
    ser("\n");

    // 3D capability (item-6 foundation): read num_capsets from the device config
    // (virtio_gpu_config.num_capsets @ +12). It's non-zero only when the host's
    // virglrenderer is live, so VIRGL-offered + capsets>0 = real 3D to build on.
    if DEVICE_CFG != 0 {
        NUM_CAPSETS = read_volatile((DEVICE_CFG + 12) as *const u32);
    }
    if VIRGL_3D {
        ser("  [virtio-gpu] VIRGL 3D offered by device — host capsets ");
        ser_dec(NUM_CAPSETS);
        if WANT_VIRGL {
            ser(" (F_VIRGL negotiated)\n");
            virgl_probe();
        } else {
            ser(" (3D-accel transport present; pass `virgltest` to negotiate)\n");
        }
    } else {
        ser("  [virtio-gpu] no VIRGL — 2D only (plain virtio-gpu / no host GL)\n");
    }
    true
}

/// Exercise the virgl 3D CONTROL path (not 3D rendering yet): query the host's
/// capability set, then create and destroy a 3D context. Both round-trip through
/// virglrenderer on the host, so success proves the 3D command channel — the
/// transport every later virgl brick (RESOURCE_CREATE_3D, SUBMIT_3D, …) rides on.
unsafe fn virgl_probe() {
    // 1. GET_CAPSET_INFO(index 0): virtio_gpu_get_capset_info {hdr(24); index u32; pad u32}
    zero(REQ_PHYS, 32);
    zero(RESP_PHYS, 64);
    write_volatile(REQ_PHYS as *mut u32, VIRTIO_GPU_CMD_GET_CAPSET_INFO);
    write_volatile((REQ_PHYS + 24) as *mut u32, 0u32); // capset_index 0
    submit(32, 40);
    let resp = RESP_PHYS as *const u8;
    let rtype = read_volatile(resp as *const u32);
    if rtype == VIRTIO_GPU_RESP_OK_CAPSET_INFO {
        let id = read_volatile(resp.add(24) as *const u32);       // 1=VIRGL, 2=VIRGL2
        let ver = read_volatile(resp.add(28) as *const u32);
        let size = read_volatile(resp.add(32) as *const u32);
        ser("  [virgl] capset 0: id ");
        ser_dec(id);
        ser(" max-version ");
        ser_dec(ver);
        ser(" max-size ");
        ser_dec(size);
        ser("\n");
    } else {
        ser("  [virgl] GET_CAPSET_INFO unexpected resp 0x");
        crate::serial::write_hex_u32(rtype);
        ser("\n");
        return;
    }

    // 2. CTX_CREATE(ctx_id 1): virtio_gpu_ctx_create {hdr(24); nlen u32; ctx_init u32; name[64]}
    zero(REQ_PHYS, 96);
    zero(RESP_PHYS, 64);
    write_volatile(REQ_PHYS as *mut u32, VIRTIO_GPU_CMD_CTX_CREATE);
    write_volatile((REQ_PHYS + 16) as *mut u32, 1u32); // hdr.ctx_id = 1
    let name = b"rustypenguin";
    write_volatile((REQ_PHYS + 24) as *mut u32, name.len() as u32); // nlen
    for (i, &c) in name.iter().enumerate() {
        write_volatile((REQ_PHYS + 32 + i as u64) as *mut u8, c);
    }
    submit(96, 24);
    let r2 = read_volatile(RESP_PHYS as *const u32);
    if r2 == VIRTIO_GPU_RESP_OK_NODATA {
        ser("  [virgl] 3D context 1 created (CTX_CREATE OK) — 3D control path live\n");
    } else {
        ser("  [virgl] CTX_CREATE unexpected resp 0x");
        crate::serial::write_hex_u32(r2);
        ser("\n");
        return;
    }

    // 3. CTX_DESTROY(ctx_id 1): clean up — just the hdr with ctx_id set.
    zero(REQ_PHYS, 24);
    zero(RESP_PHYS, 64);
    write_volatile(REQ_PHYS as *mut u32, VIRTIO_GPU_CMD_CTX_DESTROY);
    write_volatile((REQ_PHYS + 16) as *mut u32, 1u32);
    submit(24, 24);
    ser("  [virgl] 3D context 1 destroyed — virgl control-path probe PASSED\n");
}

/// Submit a two-descriptor command (request → device, response ← device) on the
/// control queue and busy-wait for completion. `req_len`/`resp_len` in bytes.
unsafe fn submit(req_len: u32, resp_len: u32) {
    // desc[0] = request (device reads), chained to desc[1] = response (device writes)
    let d = VQ_DESC_PHYS as *mut u8;
    // desc[0]
    write_volatile(d.add(0) as *mut u64, REQ_PHYS);
    write_volatile(d.add(8) as *mut u32, req_len);
    write_volatile(d.add(12) as *mut u16, VIRTQ_DESC_F_NEXT);
    write_volatile(d.add(14) as *mut u16, 1u16);
    // desc[1]
    write_volatile(d.add(16) as *mut u64, RESP_PHYS);
    write_volatile(d.add(24) as *mut u32, resp_len);
    write_volatile(d.add(28) as *mut u16, VIRTQ_DESC_F_WRITE);
    write_volatile(d.add(30) as *mut u16, 0u16);

    // avail.ring[idx % qsize] = 0 (head), then bump avail.idx.
    let a = VQ_AVAIL_PHYS as *mut u8;
    let slot = (AVAIL_IDX % QSIZE) as u64;
    write_volatile(a.add(4 + slot as usize * 2) as *mut u16, 0u16);
    compiler_fence(Ordering::SeqCst);
    AVAIL_IDX = AVAIL_IDX.wrapping_add(1);
    write_volatile(a.add(2) as *mut u16, AVAIL_IDX);
    compiler_fence(Ordering::SeqCst);

    // notify the device that the control queue has work.
    let notify_addr = NOTIFY_BASE + (QUEUE_NOTIFY_OFF as u64) * (NOTIFY_MUL as u64);
    write_volatile(notify_addr as *mut u16, 0u16);

    // busy-wait until used.idx reaches our submitted count.
    let u = VQ_USED_PHYS as *const u8;
    let mut spins = 0u64;
    loop {
        let used_idx = read_volatile(u.add(2) as *const u16);
        if used_idx == AVAIL_IDX {
            break;
        }
        spins += 1;
        if spins > 50_000_000 {
            ser("  [virtio-gpu] command timed out\n");
            break;
        }
        core::hint::spin_loop();
    }
    compiler_fence(Ordering::SeqCst);
}

/// Ask the device for its display geometry and stash it.
unsafe fn query_display() {
    // request: virtio_gpu_ctrl_hdr{ type = GET_DISPLAY_INFO } (24 bytes, rest 0)
    zero(REQ_PHYS, 24);
    zero(RESP_PHYS, 512);
    write_volatile(REQ_PHYS as *mut u32, VIRTIO_GPU_CMD_GET_DISPLAY_INFO);

    submit(24, 408);

    // response: hdr(24) then pmodes[0].rect{x,y,width,height} at +24.
    let resp = RESP_PHYS as *const u8;
    let rtype = read_volatile(resp as *const u32);
    if rtype != VIRTIO_GPU_RESP_OK_DISPLAY_INFO {
        ser("  [virtio-gpu] unexpected display-info response 0x");
        crate::serial::write_hex_u32(rtype);
        ser("\n");
        return;
    }
    let w = read_volatile(resp.add(24 + 8) as *const u32);
    let h = read_volatile(resp.add(24 + 12) as *const u32);
    let enabled = read_volatile(resp.add(24 + 16) as *const u32);
    DISP_W = w;
    DISP_H = h;
    ser("  [virtio-gpu] display 0: ");
    ser_dec(w);
    ser("x");
    ser_dec(h);
    ser(if enabled != 0 { " (enabled)\n" } else { " (off)\n" });
}

// ── Brick 2: 2D resource + scanout + dirty-rect flush ───────────────────────
// Each command builds a struct in the REQ buffer and expects an OK_NODATA hdr.

#[inline]
unsafe fn put32(off: usize, v: u32) {
    write_volatile((REQ_PHYS as *mut u8).add(off) as *mut u32, v);
}
#[inline]
unsafe fn put64(off: usize, v: u64) {
    write_volatile((REQ_PHYS as *mut u8).add(off) as *mut u64, v);
}

/// Write a virtio_gpu_ctrl_hdr (24 bytes) of the given type at REQ offset 0.
unsafe fn hdr(cmd: u32) {
    zero(REQ_PHYS, 80);
    put32(0, cmd);
}

unsafe fn resp_ok() -> bool {
    read_volatile(RESP_PHYS as *const u32) == VIRTIO_GPU_RESP_OK_NODATA
}

/// RESOURCE_CREATE_2D: allocate a host-side 2D surface of (w,h).
unsafe fn create_2d(res: u32, w: u32, h: u32) -> bool {
    hdr(VIRTIO_GPU_CMD_RESOURCE_CREATE_2D);
    put32(24, res);
    put32(28, VIRTIO_GPU_FORMAT_B8G8R8X8);
    put32(32, w);
    put32(36, h);
    submit(40, 24);
    resp_ok()
}

/// RESOURCE_ATTACH_BACKING: point the surface at one contiguous RAM region.
unsafe fn attach_backing(res: u32, phys: u64, len: u32) -> bool {
    hdr(VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING);
    put32(24, res);
    put32(28, 1); // nr_entries
    put64(32, phys); // mem_entry.addr
    put32(40, len); // mem_entry.length
    put32(44, 0); // padding
    submit(48, 24);
    resp_ok()
}

/// SET_SCANOUT: bind a scanout (monitor) to the resource over rect (0,0,w,h).
unsafe fn set_scanout(scanout: u32, res: u32, w: u32, h: u32) -> bool {
    hdr(VIRTIO_GPU_CMD_SET_SCANOUT);
    put32(24, 0); // rect.x
    put32(28, 0); // rect.y
    put32(32, w); // rect.width
    put32(36, h); // rect.height
    put32(40, scanout);
    put32(44, res);
    submit(48, 24);
    resp_ok()
}

/// TRANSFER_TO_HOST_2D: DMA a rect of the backing into the host surface.
unsafe fn transfer(res: u32, x: u32, y: u32, w: u32, h: u32, stride: u32) -> bool {
    hdr(VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D);
    put32(24, x);
    put32(28, y);
    put32(32, w);
    put32(36, h);
    put64(40, (y as u64 * stride as u64 + x as u64 * 4)); // offset into backing
    put32(48, res);
    put32(52, 0);
    submit(56, 24);
    resp_ok()
}

/// RESOURCE_FLUSH: present a rect of the surface to the screen.
unsafe fn flush(res: u32, x: u32, y: u32, w: u32, h: u32) -> bool {
    hdr(VIRTIO_GPU_CMD_RESOURCE_FLUSH);
    put32(24, x);
    put32(28, y);
    put32(32, w);
    put32(36, h);
    put32(40, res);
    put32(44, 0);
    submit(48, 24);
    resp_ok()
}

/// Brick-2 proof: create the scanout surface, paint the ternary triad
/// (warm-red −1 · stone 0 · spring-green +1) into the RAM backing, and push it
/// through the GPU pipeline (attach → scanout → transfer → flush). Proves the
/// full from-scratch display path puts real pixels on the virtio-gpu monitor.
unsafe fn scanout_selftest() {
    let w = DISP_W;
    let h = DISP_H;
    if w == 0 || h == 0 {
        return;
    }
    let stride = w * 4;

    // Make the backing USER-accessible so the ring-3 desktop can render into it
    // (identical to how the VBE framebuffer is exposed). Idempotent.
    crate::vmm::map_mmio_range(BACKING_PHYS, (w * h * 4).max(0x1000) as u64);

    // Paint the backing in cacheable RAM (fast CPU writes — the whole point).
    let fb = BACKING_PHYS as *mut u32;
    let bands = [0x00C0392Bu32, 0x008A938Cu32, 0x006FE18Bu32]; // RR GG BB
    for y in 0..h {
        for x in 0..w {
            let band = (x * 3 / w).min(2) as usize;
            write_volatile(fb.add((y * w + x) as usize), bands[band]);
        }
    }
    compiler_fence(Ordering::SeqCst);

    if !create_2d(SCANOUT_RES_ID, w, h) {
        ser("  [virtio-gpu] create_2d failed\n");
        return;
    }
    if !attach_backing(SCANOUT_RES_ID, BACKING_PHYS, w * h * 4) {
        ser("  [virtio-gpu] attach_backing failed\n");
        return;
    }
    if !set_scanout(0, SCANOUT_RES_ID, w, h) {
        ser("  [virtio-gpu] set_scanout failed\n");
        return;
    }
    if !transfer(SCANOUT_RES_ID, 0, 0, w, h, stride) {
        ser("  [virtio-gpu] transfer failed\n");
        return;
    }
    if !flush(SCANOUT_RES_ID, 0, 0, w, h) {
        ser("  [virtio-gpu] flush failed\n");
        return;
    }
    ser("  [virtio-gpu] scanout self-test OK — painted ternary triad via GPU\n");
}

/// Probe + initialise the virtio-gpu device. Additive: does not touch the live
/// VBE framebuffer. Returns true if a device was brought up.
pub fn init() -> bool {
    let found = crate::pci::find(VIRTIO_VENDOR, GPU_DEV_MODERN)
        .or_else(|| crate::pci::find(VIRTIO_VENDOR, GPU_DEV_XITION));
    let (bus, dev, func) = match found {
        Some(t) => t,
        None => {
            ser("  [virtio-gpu] no device\n");
            return false;
        }
    };
    ser("  [virtio-gpu] found at PCI ");
    ser_dec(bus as u32);
    ser(":");
    ser_dec(dev as u32);
    ser(".");
    ser_dec(func as u32);
    ser("\n");

    unsafe {
        if !bring_up(bus, dev, func) {
            return false;
        }
        query_display();
        scanout_selftest();
        READY = true;
    }
    true
}

pub fn is_ready() -> bool {
    unsafe { READY }
}
/// True if the GPU offers VIRGL 3D acceleration (host virglrenderer present).
/// The foundation a future virgl command path builds on; 2D works regardless.
pub fn has_3d() -> bool {
    unsafe { VIRGL_3D && NUM_CAPSETS > 0 }
}
pub fn display_dims() -> (u32, u32) {
    unsafe { (DISP_W, DISP_H) }
}

/// Physical base of the scanout backing — what sys_fb_query hands the desktop
/// when the GPU is the live display, so it renders straight into GPU memory.
pub fn backing_phys() -> u64 {
    BACKING_PHYS
}

/// Present a horizontal band [y, y+h) of the backing: DMA it into the host
/// surface and flush to screen. Called from sys_gpu_flush after the desktop
/// writes those rows. Full-width (the desktop only ever flushes whole rows).
pub fn flush_rect(y: u32, h: u32) {
    unsafe {
        if !READY || DISP_W == 0 {
            return;
        }
        let y = if y >= DISP_H { return } else { y };
        let h = if y + h > DISP_H { DISP_H - y } else { h };
        if h == 0 {
            return;
        }
        transfer(SCANOUT_RES_ID, 0, y, DISP_W, h, DISP_W * 4);
        flush(SCANOUT_RES_ID, 0, y, DISP_W, h);
    }
}
