// rp-install — install Rusty Penguin (Linux track) to a disk so it boots
// standalone without the ISO. Run from the live environment:
//
//     rp-install /dev/vda
//
// Layout written to the target (GPT):
//   p1  RPESP   FAT32  — /EFI/BOOT/BOOTX64.EFI (standalone GRUB) + vmlinuz + initrd.img
//   p2  RPDATA  ext4   — persistent root; init mounts it at /home on boot
//
// A target disk MUST be passed explicitly — the installer never guesses, since
// it repartitions (destroys) the device.

use std::path::Path;
use std::process::Command;

const ESP_MNT: &str = "/mnt/esp";
const CD_MNT: &str = "/mnt/cdrom";
const BOOTX64: &str = "/usr/lib/rp/BOOTX64.EFI";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let target = match args.get(1).map(|s| s.as_str()) {
        Some(t) if t.starts_with("/dev/") => t.to_string(),
        _ => { usage(); std::process::exit(2); }
    };

    println!("Rusty Penguin installer");
    println!("Target: {}  (this will ERASE it)", target);

    if let Err(e) = install(&target) {
        eprintln!("rp-install: FAILED: {}", e);
        std::process::exit(1);
    }
    println!("\nInstall complete. Remove the ISO and reboot — Rusty Penguin will");
    println!("boot from {} with a persistent root.", target);
}

fn usage() {
    eprintln!("usage: rp-install /dev/<disk>   (e.g. /dev/vda, /dev/sda, /dev/nvme0n1)");
    eprintln!("\nCandidate disks:");
    if let Ok(rd) = std::fs::read_dir("/sys/class/block") {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with("loop") || n.starts_with("ram") || n.starts_with("sr") { continue; }
            if Path::new(&format!("/sys/class/block/{}/partition", n)).exists() { continue; }
            eprintln!("  /dev/{}", n);
        }
    }
}

fn part(disk: &str, n: u32) -> String {
    let last = disk.chars().last().unwrap_or('a');
    if last.is_ascii_digit() { format!("{}p{}", disk, n) } else { format!("{}{}", disk, n) }
}

fn run(cmd: &str, args: &[&str]) -> Result<(), String> {
    let st = Command::new(cmd).args(args).status()
        .map_err(|e| format!("could not run {}: {}", cmd, e))?;
    if st.success() { Ok(()) } else { Err(format!("{} {:?} exited {:?}", cmd, args, st.code())) }
}

fn mount(src: &str, dst: &str, fstype: &str, ro: bool) -> Result<(), String> {
    use nix::mount::{mount, MsFlags};
    let _ = std::fs::create_dir_all(dst);
    let flags = if ro { MsFlags::MS_RDONLY } else { MsFlags::empty() };
    mount(Some(src), dst, Some(fstype), flags, None::<&str>)
        .map_err(|e| format!("mount {} ({}) on {}: {}", src, fstype, dst, e))
}

fn umount(dst: &str) {
    let _ = nix::mount::umount(dst);
}

fn install(disk: &str) -> Result<(), String> {
    if !Path::new(disk).exists() {
        return Err(format!("{} does not exist", disk));
    }
    if !Path::new(BOOTX64).exists() {
        return Err(format!("missing bootloader image {} (build bundling issue)", BOOTX64));
    }

    // 1. Mount the install media to copy the kernel + initrd. isofs (CD) and
    //    nls_iso8859-1 (vfat's IO charset, for the ESP later) are modules.
    println!("[1/6] Mounting install media...");
    for m in ["/lib/modules/isofs.ko", "/lib/modules/nls_iso8859-1.ko"] {
        let _ = Command::new("/bin/busybox").args(["insmod", m]).status();
    }
    let cd = ["/dev/sr0", "/dev/sr1", "/dev/cdrom"].iter()
        .find(|d| Path::new(d).exists())
        .ok_or("no CD-ROM device found")?;
    mount(cd, CD_MNT, "iso9660", true)?;
    let kernel = format!("{}/boot/vmlinuz", CD_MNT);
    let initrd = format!("{}/boot/initrd.img", CD_MNT);
    if !Path::new(&kernel).exists() || !Path::new(&initrd).exists() {
        umount(CD_MNT);
        return Err("kernel/initrd not found on install media".to_string());
    }

    // 2. Partition: GPT with an ESP and an ext4 root labeled RPDATA.
    println!("[2/6] Partitioning {} (GPT: ESP + RPDATA)...", disk);
    run("/bin/sgdisk", &["--zap-all", disk])?;
    run("/bin/sgdisk", &[
        "-n", "1:0:+256M", "-t", "1:ef00", "-c", "1:RPESP",
        "-n", "2:0:0",     "-t", "2:8300", "-c", "2:RPDATA",
        disk,
    ])?;
    // Nudge the kernel to re-read the new partition table, then wait for nodes.
    let _ = Command::new("/bin/busybox").args(["blockdev", "--rereadpt", disk]).status();
    let p1 = part(disk, 1);
    let p2 = part(disk, 2);
    for _ in 0..50 {
        if Path::new(&p1).exists() && Path::new(&p2).exists() { break; }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if !Path::new(&p1).exists() || !Path::new(&p2).exists() {
        umount(CD_MNT);
        return Err(format!("partitions {} / {} did not appear", p1, p2));
    }

    // 3. Format the ESP (FAT32) and the root (ext4, busybox mke2fs).
    println!("[3/6] Formatting {} (FAT32) and {} (ext4)...", p1, p2);
    run("/bin/mkfs.fat", &["-F", "32", "-n", "RPESP", &p1])?;
    run("/bin/busybox", &["mke2fs", "-F", "-j", "-L", "RPDATA", &p2])?;

    // 4. Populate the ESP: bootloader + kernel + initrd.
    println!("[4/6] Installing bootloader + kernel to ESP...");
    mount(&p1, ESP_MNT, "vfat", false)?;
    std::fs::create_dir_all(format!("{}/EFI/BOOT", ESP_MNT)).map_err(|e| e.to_string())?;
    copy(BOOTX64, &format!("{}/EFI/BOOT/BOOTX64.EFI", ESP_MNT))?;
    copy(&kernel, &format!("{}/vmlinuz", ESP_MNT))?;
    copy(&initrd, &format!("{}/initrd.img", ESP_MNT))?;

    // 5. Flush everything to disk.
    println!("[5/6] Syncing...");
    umount(ESP_MNT);
    umount(CD_MNT);
    nix::unistd::sync();

    // 6. Done. The RPDATA partition is left empty; init provisions/uses it.
    println!("[6/6] Done.");
    Ok(())
}

fn copy(src: &str, dst: &str) -> Result<(), String> {
    let bytes = std::fs::copy(src, dst)
        .map_err(|e| format!("copy {} -> {}: {}", src, dst, e))?;
    println!("       {} -> {} ({} KiB)", src, dst, bytes / 1024);
    Ok(())
}
