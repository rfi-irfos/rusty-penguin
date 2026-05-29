use std::process::Command;
use std::fs;
use std::path::Path;
use ternary_core::{Trit, Tryte};

fn main() {
    println!("Rusty Penguin init (PID 1) starting...");

    mount_pseudo_filesystems();
    setup_environment();

    let console = console_mode();

    // Storage is a TERNARY subsystem, not a binary present/absent flag:
    //   +1 Active     — disk present & formatted → mounted, persistent
    //    0 Dormant    — disk present but blank   → provisioned, then activated
    //   -1 Suppressed — no disk / mount failed   → ephemeral boot, no persistence
    //
    // In console/install mode we do NOT auto-provision: the installer owns the
    // disks, and grabbing/formatting the install target here would fight it.
    let storage = if console {
        eprintln!("[init] console mode — skipping disk auto-provision (installer owns disks)");
        Trit::Zero
    } else {
        setup_persistence()
    };
    log_storage_state(storage);

    // Networking is also a ternary subsystem:
    //   +1 Active     — link up and a DHCP lease was obtained
    //    0 Dormant    — interface present and up, but no lease yet
    //   -1 Suppressed — no network device
    let net = bring_up_network();
    log_net_state(net);

    setup_home_directory();

    if storage == Trit::Pos {
        let boots = update_boot_record(storage, net);
        eprintln!("[init] persistent boot #{} (record: ~/.rusty/boot.tern)", boots);
    }

    // Console/installer mode: `rp.console` (or `rp.install`) on the kernel
    // cmdline boots straight to a text shell instead of the desktop — used to
    // run `rp-install <disk>`, and as a rescue console. RP_RECOVERY stops the
    // shell from bouncing back into the desktop.
    let result = if web_mode() {
        // rp.web — boot the X11 web session (Xorg on /dev/fb0 + a real GUI app)
        // instead of the bespoke desktop. The path to running Firefox/Chrome.
        eprintln!("[init] web mode (rp.web) — starting X11 session...");
        load_input_modules();
        launch_x_session()
    } else if console {
        eprintln!("[init] console mode (rp.console) — run `rp-install /dev/<disk>` to install");
        std::env::set_var("RP_RECOVERY", "1");
        launch_shell()
    } else {
        // Phase 1 substrate: try the graphical desktop first; init's
        // launch_session falls back to a shell if it can't start.
        launch_session()
    };
    match result {
        Ok(()) => println!("[init] Session exited cleanly. Halting init loop."),
        Err(e) => eprintln!("[init] Session launch failed: {}", e),
    }
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn console_mode() -> bool {
    fs::read_to_string("/proc/cmdline")
        .map(|c| c.contains("rp.console") || c.contains("rp.install"))
        .unwrap_or(false)
}

fn web_mode() -> bool {
    fs::read_to_string("/proc/cmdline").map(|c| c.contains("rp.web")).unwrap_or(false)
}

/// Load input modules so Xorg/libinput sees a keyboard+mouse via /dev/input.
fn load_input_modules() {
    for m in ["/lib/modules/virtio_input.ko"] {
        let _ = Command::new("/bin/busybox").args(["insmod", m]).status();
    }
}

/// Launch the X11 web session via the bundled rootfs launcher.
fn launch_x_session() -> Result<(), Box<dyn std::error::Error>> {
    if !Path::new("/start-x.sh").exists() {
        return Err("web rootfs not present (/start-x.sh missing)".into());
    }
    let status = Command::new("/bin/busybox").args(["sh", "/start-x.sh"]).status()?;
    eprintln!("[init] X session exited: {:?}", status.code());
    Ok(())
}

fn launch_shell() -> Result<(), Box<dyn std::error::Error>> {
    for path in ["/bin/shell", "/usr/local/bin/shell", "/bin/psh", "/usr/local/bin/psh"] {
        if Path::new(path).exists() {
            let status = Command::new(path).status()?;
            println!("[init] Shell exited with status: {:?}", status.code());
            return Ok(());
        }
    }
    Err("no shell binary found".into())
}

fn setup_environment() {
    // Set basic environment variables
    std::env::set_var("PATH", "/opt/rusty-penguin/bin:/bin:/usr/local/bin:/usr/bin");
    std::env::set_var("HOME", "/home/rusty-penguin");
    std::env::set_var("SHELL", "/bin/psh");
    std::env::set_var("TERM", "xterm");

    println!("[init] Environment initialized");
}

// ─── Persistence: a ternary storage subsystem ────────────────────────────────
// The live ISO is otherwise ephemeral (initramfs only). This makes Rusty Penguin
// daily-drivable: a writable disk is mounted at /home so user files and config
// survive reboots. Disk state is a Trit (+1/0/-1), and the boot record is a
// first-class `.tern` file (balanced ternary, via ternary-core).

const PERSIST_MNT: &str = "/persist";
const PERSIST_HOME: &str = "/persist/home/rusty-penguin";

/// Mount the kernel pseudo-filesystems a PID 1 needs. devtmpfs gives us the
/// block-device nodes (/dev/vda …) that disk detection and mke2fs require.
fn mount_pseudo_filesystems() {
    use nix::mount::{mount, MsFlags};
    let mounts = [
        ("proc",     "/proc", "proc"),
        ("sysfs",    "/sys",  "sysfs"),
        ("devtmpfs", "/dev",  "devtmpfs"),
        ("tmpfs",    "/tmp",  "tmpfs"),
    ];
    for (src, target, fstype) in mounts {
        let _ = fs::create_dir_all(target);
        let _ = mount(Some(src), target, Some(fstype), MsFlags::empty(), None::<&str>);
    }
}

fn run_ok(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd).args(args).status().map(|s| s.success()).unwrap_or(false)
}

/// Enumerate real block devices from /sys/class/block as (name, is_partition),
/// skipping loop/ram/CD devices.
fn list_block_devices() -> Vec<(String, bool)> {
    let mut v = Vec::new();
    if let Ok(rd) = fs::read_dir("/sys/class/block") {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("loop") || name.starts_with("ram") || name.starts_with("sr") {
                continue;
            }
            let is_part = Path::new(&format!("/sys/class/block/{}/partition", name)).exists();
            v.push((name, is_part));
        }
    }
    v
}

/// Read the ext2/3/4 volume label directly from the superblock (no blkid needed).
/// Superblock starts at byte 1024; s_magic at +0x38 (0xEF53), s_volume_name at +0x78.
fn ext_label(dev: &str) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = fs::File::open(dev).ok()?;
    let mut magic = [0u8; 2];
    f.seek(SeekFrom::Start(1024 + 0x38)).ok()?;
    f.read_exact(&mut magic).ok()?;
    if u16::from_le_bytes(magic) != 0xEF53 { return None; } // not an ext filesystem
    let mut label = [0u8; 16];
    f.seek(SeekFrom::Start(1024 + 0x78)).ok()?;
    f.read_exact(&mut label).ok()?;
    let end = label.iter().position(|&b| b == 0).unwrap_or(16);
    Some(String::from_utf8_lossy(&label[..end]).into_owned())
}

/// Find a partition holding our RPDATA-labeled filesystem (an installed system).
fn find_rpdata_partition() -> Option<String> {
    for (name, is_part) in list_block_devices() {
        if !is_part { continue; }
        let dev = format!("/dev/{}", name);
        if ext_label(&dev).as_deref() == Some("RPDATA") { return Some(dev); }
    }
    None
}

/// Find a whole disk with NO partition table — the only thing safe to
/// auto-provision. A disk that already has partitions belongs to an installed
/// system; reformatting it would destroy the partition table (data loss).
fn find_blank_disk() -> Option<String> {
    let devs = list_block_devices();
    for (name, is_part) in &devs {
        if *is_part { continue; }
        let has_children = devs.iter().any(|(n, p)| *p && n.starts_with(name.as_str()));
        if has_children { continue; }
        return Some(format!("/dev/{}", name));
    }
    None
}

fn mount_fs(dev: &str, target: &str, fstype: &str) -> bool {
    use nix::mount::{mount, MsFlags};
    let _ = fs::create_dir_all(target);
    mount(Some(dev), target, Some(fstype), MsFlags::empty(), None::<&str>).is_ok()
}

fn mount_ext4(dev: &str, target: &str) -> bool {
    mount_fs(dev, target, "ext4")
}

fn bind_mount(src: &str, dst: &str) -> bool {
    use nix::mount::{mount, MsFlags};
    let _ = fs::create_dir_all(src);
    let _ = fs::create_dir_all(dst);
    mount(Some(src), dst, None::<&str>, MsFlags::MS_BIND, None::<&str>).is_ok()
}

/// Bring up persistent storage. Returns the storage state as a Trit.
///
/// Resolution order (partition-aware so it never clobbers an installed system):
///   1. an existing RPDATA-labeled partition  → mount it (installed-to-disk)
///   2. a whole BLANK disk (no partition table) → mount or auto-provision (live)
///   3. nothing writable                        → suppressed (ephemeral)
fn setup_persistence() -> Trit {
    // 1. Installed system: a partition labeled RPDATA.
    if let Some(part) = find_rpdata_partition() {
        if mount_ext4(&part, PERSIST_MNT) || mount_fs(&part, PERSIST_MNT, "ext2") {
            finish_persist_setup();
            return Trit::Pos;
        }
    }

    // 2. Live system: a genuinely blank whole disk we may provision. Never a
    //    disk that already has a partition table (that would be data loss).
    let disk = match find_blank_disk() {
        Some(d) => d,
        None => return Trit::Neg, // no safe writable disk → ephemeral boot
    };

    // Maybe it already holds a whole-device fs from a previous live boot.
    if mount_ext4(&disk, PERSIST_MNT) || mount_fs(&disk, PERSIST_MNT, "ext2") {
        finish_persist_setup();
        return Trit::Pos;
    }

    // 0 → dormant: blank. Provision it. busybox mke2fs is a static,
    // dependency-free formatter; it doesn't take util-linux's `-t`, so we ask
    // for an ext-family fs with journaling via `-j` (ext3 layout, which the
    // kernel's ext4 driver mounts fine). `-F` forces a whole-device fs.
    eprintln!("[init] storage 0 (dormant): provisioning {} ...", disk);
    let mkfs = Command::new("/bin/busybox")
        .args(["mke2fs", "-F", "-j", "-L", "RPDATA", &disk])
        .output();
    match mkfs {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            eprintln!("[init] mke2fs failed (code {:?}): {}",
                o.status.code(), String::from_utf8_lossy(&o.stderr).trim());
            return Trit::Neg;
        }
        Err(e) => { eprintln!("[init] mke2fs not runnable: {}", e); return Trit::Neg; }
    }
    if mount_ext4(&disk, PERSIST_MNT) || mount_fs(&disk, PERSIST_MNT, "ext2") {
        finish_persist_setup();
        Trit::Pos
    } else {
        eprintln!("[init] provisioned {} but mount failed (ext4/ext2 driver missing?)", disk);
        Trit::Neg
    }
}

/// Bind persistent dirs over the live root so user data and installed packages
/// land on the disk: /home (files + config) and /opt (rpm packages).
fn finish_persist_setup() {
    let _ = fs::create_dir_all(PERSIST_HOME);
    let _ = fs::create_dir_all("/home/rusty-penguin");
    bind_mount("/persist/home", "/home");

    // Installed packages (rpm → /opt/rusty-penguin) should survive reboots too.
    let _ = fs::create_dir_all("/persist/opt");
    let _ = fs::create_dir_all("/opt");
    if bind_mount("/persist/opt", "/opt") {
        eprintln!("[init] /opt is persistent (installed packages survive reboot)");
    }
}

// ─── Networking: another ternary subsystem ───────────────────────────────────

fn find_netif() -> Option<String> {
    for e in fs::read_dir("/sys/class/net").ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name != "lo" { return Some(name); }
    }
    None
}

/// Bring the first non-loopback interface up and acquire a DHCP lease via the
/// bundled static busybox udhcpc. Returns link state as a Trit.
fn bring_up_network() -> Trit {
    let iface = match find_netif() {
        Some(i) => i,
        None => return Trit::Neg, // suppressed — no NIC
    };
    let _ = Command::new("/bin/busybox").args(["ip", "link", "set", &iface, "up"]).status();

    // One-shot DHCP: -q quit after lease, -n exit if none, -t/-T retry tuning.
    let leased = Command::new("/bin/busybox")
        .args(["udhcpc", "-i", &iface, "-s", "/etc/udhcpc.script", "-q", "-n", "-t", "5", "-T", "2"])
        .status().map(|s| s.success()).unwrap_or(false);

    if !leased {
        return Trit::Zero; // link up, no lease (dormant)
    }
    if let Some(ip) = iface_ipv4(&iface) {
        eprintln!("[init] net iface {} → {}", iface, ip);
    }
    // Reachability probe: a lease alone doesn't mean the path works. Ping the
    // default gateway once so +1 means "network actually reachable".
    match default_gateway() {
        Some(gw) => {
            let reachable = Command::new("/bin/busybox")
                .args(["ping", "-c", "1", "-W", "2", &gw])
                .status().map(|s| s.success()).unwrap_or(false);
            if reachable {
                eprintln!("[init] gateway {} reachable", gw);
                Trit::Pos
            } else {
                eprintln!("[init] gateway {} unreachable (lease ok)", gw);
                Trit::Zero
            }
        }
        None => Trit::Pos, // leased but no default route info; treat as up
    }
}

/// Parse the default-route gateway from `busybox ip route`.
fn default_gateway() -> Option<String> {
    let out = Command::new("/bin/busybox").args(["ip", "route", "show", "default"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut it = text.split_whitespace();
    while let Some(tok) = it.next() {
        if tok == "via" { return it.next().map(|s| s.to_string()); }
    }
    None
}

/// Read the interface's IPv4 from `busybox ip -4 addr`.
fn iface_ipv4(iface: &str) -> Option<String> {
    let out = Command::new("/bin/busybox").args(["ip", "-4", "addr", "show", iface]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("inet ") {
            return Some(rest.split_whitespace().next()?.to_string());
        }
    }
    None
}

fn log_net_state(t: Trit) {
    let (sym, label) = match t {
        Trit::Pos  => ("+1", "ACTIVE — DHCP lease acquired"),
        Trit::Zero => ("0",  "DORMANT — link up, no lease"),
        Trit::Neg  => ("-1", "SUPPRESSED — no network device"),
    };
    eprintln!("[init] network {} : {}", sym, label);
}

fn log_storage_state(t: Trit) {
    let (sym, label) = match t {
        Trit::Pos  => ("+1", "ACTIVE — persistent disk mounted at /home"),
        Trit::Zero => ("0",  "DORMANT — provisioning"),
        Trit::Neg  => ("-1", "SUPPRESSED — ephemeral boot (no writable disk)"),
    };
    eprintln!("[init] storage {} : {}", sym, label);
}

/// Read, increment, and persist the boot counter in a `.tern` file. Returns the
/// new count. The file is balanced-ternary aware: the count is also stored as a
/// 9-trit Tryte (the OS's native numeric form), making `.tern` first-class.
fn update_boot_record(storage: Trit, net: Trit) -> i32 {
    let dir = format!("{}/.rusty", PERSIST_HOME);
    let _ = fs::create_dir_all(&dir);
    let path = format!("{}/boot.tern", dir);

    let prev = fs::read_to_string(&path).ok()
        .and_then(|s| parse_tern_int(&s, "boots"))
        .unwrap_or(0);
    let n = prev + 1;

    let _ = fs::write(&path, render_boot_tern(storage, net, n));
    // Flush to the block device. Without this the write lives only in the page
    // cache and is lost on power-off (no clean unmount yet) — the counter would
    // never advance. sync(2) pushes all dirty buffers to disk.
    nix::unistd::sync();
    n
}

/// Render a `.tern` document. Format (one record per line):
///   `@key  <trit|int>  [tryte]`
/// where a trit is `+`/`0`/`-` and a tryte is 9 balanced-ternary digits.
fn render_boot_tern(storage: Trit, net: Trit, boots: i32) -> String {
    let tryte = Tryte::from_i32(boots);
    format!(
        "# boot.tern — Rusty Penguin boot record (balanced ternary)\n\
         # @key   value   [tryte: 9 trits, high→low, +/0/-]\n\
         @storage {}\n\
         @network {}\n\
         @boots   {}   {}\n\
         @native  ternary\n",
        trit_char(storage),
        trit_char(net),
        boots,
        tryte_str(&tryte),
    )
}

fn parse_tern_int(doc: &str, key: &str) -> Option<i32> {
    for line in doc.lines() {
        let line = line.trim();
        if line.starts_with('#') { continue; }
        let mut it = line.split_whitespace();
        if it.next() == Some(&format!("@{}", key)) {
            if let Some(v) = it.next() {
                if let Ok(n) = v.parse::<i32>() { return Some(n); }
            }
        }
    }
    None
}

fn trit_char(t: Trit) -> char {
    match t { Trit::Pos => '+', Trit::Zero => '0', Trit::Neg => '-' }
}

fn tryte_str(t: &Tryte) -> String {
    // trits() is least-significant first; print high→low for readability.
    let mut s = String::with_capacity(9);
    for trit in t.trits().iter().rev() {
        s.push(trit_char(*trit));
    }
    s
}

fn setup_home_directory() {
    let home = "/home/rusty-penguin";
    
    // Create home directory
    if !Path::new(home).exists() {
        if let Ok(_) = fs::create_dir_all(home) {
            println!("[init] Created home directory: {}", home);
        }
    }

    // Create config directory
    let config_dir = format!("{}/.config/rusty-penguin", home);
    if !Path::new(&config_dir).exists() {
        if let Ok(_) = fs::create_dir_all(&config_dir) {
            println!("[init] Created config directory: {}", config_dir);
        }
    }

    // Create user shell history file
    let history_file = format!("{}/.psh_history", home);
    if !Path::new(&history_file).exists() {
        let _ = fs::File::create(&history_file);
        println!("[init] Created history file: {}", history_file);
    }

    std::env::set_var("HOME", home);
}

fn launch_session() -> Result<(), Box<dyn std::error::Error>> {
    // Preferred: graphical desktop (rp-compositor on Linux substrate).
    let desktop_paths = ["/bin/desktop", "/usr/local/bin/desktop"];
    for path in &desktop_paths {
        if Path::new(path).exists() {
            println!("[init] Found desktop: {} — launching...", path);
            match Command::new(path).status() {
                Ok(status) if status.success() => {
                    // Clean exit (e.g. user logged out) — stop here.
                    println!("[init] Desktop exited cleanly.");
                    return Ok(());
                }
                Ok(status) => {
                    // Desktop crashed or couldn't start (no /dev/fb0, etc).
                    // Drop to a recovery console instead of leaving the user
                    // staring at a frozen system.
                    eprintln!("[init] Desktop exited with status {:?} — dropping to shell.",
                        status.code());
                    break;
                }
                Err(e) => {
                    eprintln!("[init] Desktop failed to spawn ({}). Falling back to shell.", e);
                    break;
                }
            }
        }
    }

    // Fallback: shell. The system is still usable as a text console.
    let shell_paths = ["/bin/shell", "/usr/local/bin/shell", "/bin/psh", "/usr/local/bin/psh"];
    for path in &shell_paths {
        if Path::new(path).exists() {
            println!("[init] Found shell: {} — launching...", path);
            let status = Command::new(path).status()?;
            println!("[init] Shell exited with status: {:?}", status.code());
            return Ok(());
        }
    }

    Err("No desktop or shell binary found".into())
}
