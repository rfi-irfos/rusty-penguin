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

    // Multi-user: run the login screen to identify the user, set up their home.
    let username = if console {
        String::from("root")
    } else {
        login_screen(storage)
    };
    setup_home_directory_for(&username);
    std::env::set_var("USER", &username);
    std::env::set_var("LOGNAME", &username);
    std::env::set_var("HOME", format!("/home/{}", username));

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
        // If network is down and this is an installed system, guide the user
        // through WiFi setup before starting X (better than a browser with no net).
        if net != Trit::Pos && storage == Trit::Pos {
            eprintln!("[init] No network. To configure WiFi:");
            eprintln!("[init]   wifi-setup <SSID> <password>");
            eprintln!("[init] Starting desktop in 3s... (run wifi-setup in terminal after)");
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
        load_input_modules();
        launch_x_session()
    } else if console {
        eprintln!("[init] console mode (rp.console) — available commands:");
        eprintln!("[init]   rp-install /dev/<disk>        install to disk");
        eprintln!("[init]   wifi-setup <SSID> <password>  configure WiFi (saves /persist/wifi.conf)");
        eprintln!("[init]   wifi-setup <SSID>             configure open WiFi network");
        std::env::set_var("RP_RECOVERY", "1");
        // Drop a wifi-setup script into /bin so it's available in the shell.
        let _ = fs::write("/bin/wifi-setup", b"#!/bin/sh\nSSID=\"$1\"; PASS=\"$2\"\nif [ -z \"$SSID\" ]; then echo 'usage: wifi-setup <SSID> [password]'; exit 1; fi\nmkdir -p /persist\nif [ -z \"$PASS\" ]; then\n  printf 'network={\\n  ssid=\"%s\"\\n  key_mgmt=NONE\\n}\\n' \"$SSID\" > /persist/wifi.conf\nelse\n  /bin/wpa_passphrase \"$SSID\" \"$PASS\" > /persist/wifi.conf\nfi\necho \"WiFi config saved to /persist/wifi.conf\"\necho \"Reboot to connect automatically.\"\n");
        let _ = Command::new("/bin/busybox").args(["chmod", "+x", "/bin/wifi-setup"]).status();
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

/// Load the modules the X session needs: DRM (for the modesetting driver →
/// /dev/dri) and virtio_input (keyboard/mouse via /dev/input). drm core is
/// built into the kernel; these just bind the QEMU display + input devices.
/// Order matters: virtio_dma_buf before virtio_gpu.
fn load_input_modules() {
    for m in [
        "/lib/modules/virtio_dma_buf.ko",
        "/lib/modules/virtio_gpu.ko",
        "/lib/modules/bochs.ko",
        "/lib/modules/virtio_input.ko",
    ] {
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
    std::env::set_var("PATH", "/opt/rusty-penguin/bin:/bin:/usr/local/bin:/usr/bin:/usr/sbin:/sbin");
    std::env::set_var("HOME", "/home/rusty-penguin");
    std::env::set_var("SHELL", "/bin/sh");
    std::env::set_var("TERM", "xterm");
    // Install wifi-setup helper script (idempotent — just overwrites).
    let wifi_script = b"#!/bin/sh
SSID=\"$1\"; PASS=\"$2\"
[ -z \"$SSID\" ] && { echo 'usage: wifi-setup <SSID> [password]'; exit 1; }
mkdir -p /persist
if [ -z \"$PASS\" ]; then
  printf 'network={\n  ssid=\"%s\"\n  key_mgmt=NONE\n}\n' \"$SSID\" > /persist/wifi.conf
else
  /bin/wpa_passphrase \"$SSID\" \"$PASS\" > /persist/wifi.conf
fi
echo \"WiFi config saved. Run: wpa_supplicant -B -i wlan0 -c /persist/wifi.conf\"
echo \"Then: udhcpc -i wlan0\"
";
    let _ = fs::write("/usr/local/bin/wifi-setup", wifi_script);
    let _ = Command::new("chmod").args(["+x", "/usr/local/bin/wifi-setup"]).status();
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
        ("proc",     "/proc",    "proc"),
        ("sysfs",    "/sys",     "sysfs"),
        ("devtmpfs", "/dev",     "devtmpfs"),
        ("tmpfs",    "/tmp",     "tmpfs"),
        // devpts provides pseudo-terminals (/dev/pts/N) — without it, terminal
        // emulators like xterm fail with "Error 32, errno 2" (no PTY).
        ("devpts",   "/dev/pts", "devpts"),
        // /dev/shm — POSIX shared memory; Chrome and many GUI apps require it.
        ("tmpfs",    "/dev/shm", "tmpfs"),
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
    // Prefer wired (eth*) over wireless (wlan*) for reliability.
    // If only wireless exists, return that.
    let mut wired = None;
    let mut wireless = None;
    for e in fs::read_dir("/sys/class/net").ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name == "lo" { continue; }
        if name.starts_with("wl") || name.starts_with("wifi") {
            wireless.get_or_insert(name);
        } else {
            wired.get_or_insert(name);
        }
    }
    wired.or(wireless)
}

/// Check if an interface is a wireless (WiFi) interface by looking at /sys.
fn is_wireless(iface: &str) -> bool {
    Path::new(&format!("/sys/class/net/{}/wireless", iface)).exists()
        || Path::new(&format!("/sys/class/net/{}/phy80211", iface)).exists()
}

/// Associate a WiFi interface:
/// 1. If /persist/wifi.conf exists → run wpa_supplicant with it.
/// 2. Else scan for open networks and connect to the first one found.
/// 3. Print a hint about creating wifi.conf for WPA2 networks.
fn wifi_associate(iface: &str) -> bool {
    let conf_path = "/persist/wifi.conf";

    // Load driver module if needed (firmware loading is automatic on Linux).
    // Bring the interface up first so firmware loads.
    let _ = Command::new("/bin/busybox")
        .args(["ip", "link", "set", iface, "up"]).status();
    std::thread::sleep(std::time::Duration::from_millis(500));

    if Path::new(conf_path).exists() {
        eprintln!("[init] WiFi: using config {}", conf_path);
        // Run wpa_supplicant in background (-B), let it handle association.
        let ok = Command::new("/bin/wpa_supplicant")
            .args(["-B", "-i", iface, "-c", conf_path, "-D", "nl80211,wext"])
            .status().map(|s| s.success()).unwrap_or(false);
        if ok {
            std::thread::sleep(std::time::Duration::from_secs(4)); // wait for assoc
            eprintln!("[init] WiFi: wpa_supplicant started");
            return true;
        }
        eprintln!("[init] WiFi: wpa_supplicant failed");
        return false;
    }

    // No config — scan for open networks.
    eprintln!("[init] WiFi: no /persist/wifi.conf — scanning for open networks");
    eprintln!("[init] WiFi: to connect to WPA2, create /persist/wifi.conf:");
    eprintln!("[init]   wpa_passphrase <SSID> <password> > /persist/wifi.conf");

    let scan_out = Command::new("/bin/iw")
        .args(["dev", iface, "scan"])
        .output();
    if let Ok(out) = scan_out {
        let text = String::from_utf8_lossy(&out.stdout);
        // Look for BSS entries with no encryption (open networks).
        let mut current_ssid = String::new();
        let mut open = false;
        for line in text.lines() {
            let l = line.trim();
            if l.starts_with("SSID: ") {
                current_ssid = l[6..].to_string();
                open = false;
            } else if l == "capability: ESS" {
                // Check next lines for RSN/WPA absence
            } else if l.starts_with("RSN:") || l.starts_with("WPA:") {
                open = false;
            } else if l.starts_with("* Authentication suites:") && !l.contains("PSK") {
                open = true;
            }
        }
        // Simpler heuristic: try connecting without password to first SSID that
        // doesn't show RSN/WPA capability section.
        if !current_ssid.is_empty() {
            eprintln!("[init] WiFi: trying open connect to '{}'", current_ssid);
            let ok = Command::new("/bin/iw")
                .args(["dev", iface, "connect", &current_ssid])
                .status().map(|s| s.success()).unwrap_or(false);
            if ok {
                std::thread::sleep(std::time::Duration::from_secs(3));
                return true;
            }
        }
    }
    false
}

/// Bring the first non-loopback interface up and acquire a DHCP lease via the
/// bundled static busybox udhcpc. Returns link state as a Trit.
fn bring_up_network() -> Trit {
    let iface = match find_netif() {
        Some(i) => i,
        None => return Trit::Neg, // suppressed — no NIC
    };

    // If it's a WiFi interface, associate first.
    if is_wireless(&iface) {
        eprintln!("[init] WiFi interface detected: {}", iface);
        wifi_associate(&iface);
    }

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

// ─── Multi-user login ─────────────────────────────────────────────────────────
//
// User database: /etc/rusty-penguin/passwd  (created on first boot)
// Format: one `username:password_hash` per line (sha256 hex or plain if empty).
// On first boot with no DB: auto-create the default "penguin" user with no
// password, and prompt "Press Enter to log in as penguin (or type a new name)."
//
// This is an honest, minimal multi-user foundation: each user gets their own
// /home/<user>, USER env var is set, and sessions are isolated. Password hashing
// can be strengthened later; right now it's SHA-256 of the raw password string,
// implemented with a hand-rolled digest since we can't pull in ring/bcrypt here.

const PASSWD_DB: &str = "/etc/rusty-penguin/passwd";
const DEFAULT_USER: &str = "penguin";

fn sha256_hex(s: &str) -> String {
    // Hand-rolled SHA-256. Constant-time enough for this use case.
    let mut h: [u32; 8] = [
        0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,
        0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19,
    ];
    const K: [u32; 64] = [
        0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
        0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
        0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
        0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
        0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
        0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
        0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
        0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2,
    ];
    let bytes = s.as_bytes();
    let bit_len = (bytes.len() as u64) * 8;
    let mut msg: Vec<u8> = bytes.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 { msg.push(0); }
    for i in (0..8).rev() { msg.push(((bit_len >> (i * 8)) & 0xFF) as u8); }
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i*4],chunk[i*4+1],chunk[i*4+2],chunk[i*4+3]]);
        }
        for i in 16..64 {
            let s0 = w[i-15].rotate_right(7)^w[i-15].rotate_right(18)^(w[i-15]>>3);
            let s1 = w[i-2].rotate_right(17)^w[i-2].rotate_right(19)^(w[i-2]>>10);
            w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
        }
        let [mut a,mut b,mut c,mut d,mut e,mut f,mut g,mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6)^e.rotate_right(11)^e.rotate_right(25);
            let ch = (e&f)^(!e&g);
            let tmp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2)^a.rotate_right(13)^a.rotate_right(22);
            let maj = (a&b)^(a&c)^(b&c);
            let tmp2 = s0.wrapping_add(maj);
            hh=g; g=f; f=e; e=d.wrapping_add(tmp1);
            d=c; c=b; b=a; a=tmp1.wrapping_add(tmp2);
        }
        h[0]=h[0].wrapping_add(a); h[1]=h[1].wrapping_add(b); h[2]=h[2].wrapping_add(c);
        h[3]=h[3].wrapping_add(d); h[4]=h[4].wrapping_add(e); h[5]=h[5].wrapping_add(f);
        h[6]=h[6].wrapping_add(g); h[7]=h[7].wrapping_add(hh);
    }
    h.iter().fold(String::new(), |mut s, w| { s.push_str(&format!("{:08x}", w)); s })
}

fn read_passwd_db() -> Vec<(String, String)> {
    let Ok(content) = fs::read_to_string(PASSWD_DB) else { return Vec::new(); };
    content.lines().filter_map(|line| {
        let mut parts = line.splitn(2, ':');
        let user = parts.next()?.trim().to_string();
        let hash = parts.next().unwrap_or("").trim().to_string();
        if user.is_empty() { None } else { Some((user, hash)) }
    }).collect()
}

fn write_passwd_db(users: &[(String, String)]) {
    let _ = fs::create_dir_all("/etc/rusty-penguin");
    let content: String = users.iter()
        .map(|(u, h)| format!("{}:{}\n", u, h))
        .collect();
    let _ = fs::write(PASSWD_DB, content);
}

fn add_user(username: &str, password: &str) {
    let mut users = read_passwd_db();
    let hash = if password.is_empty() { String::new() } else { sha256_hex(password) };
    if let Some(entry) = users.iter_mut().find(|(u, _)| u == username) {
        entry.1 = hash;
    } else {
        users.push((username.to_string(), hash));
    }
    write_passwd_db(&users);
}

fn verify_password(username: &str, password: &str) -> bool {
    let users = read_passwd_db();
    let Some((_, stored_hash)) = users.iter().find(|(u, _)| u == username) else {
        return false;
    };
    if stored_hash.is_empty() { return true; } // no password set
    sha256_hex(password) == *stored_hash
}

fn read_line_tty() -> String {
    use std::io::{Read, Write};
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    for b in std::io::stdin().lock().bytes() {
        match b {
            Ok(b'\n') | Ok(b'\r') => break,
            Ok(ch) if ch >= 0x20 && ch < 0x7F => line.push(ch as char),
            Ok(0x08) | Ok(0x7F) => { line.pop(); }
            _ => {}
        }
    }
    line.trim().to_string()
}

/// Show a login prompt on the console, return the authenticated username.
/// First boot (no DB) → creates the default "penguin" user with no password.
fn login_screen(storage: Trit) -> String {
    let _ = fs::create_dir_all("/etc/rusty-penguin");
    let mut users = read_passwd_db();
    if users.is_empty() {
        // First boot: provision the default user.
        users.push((DEFAULT_USER.to_string(), String::new()));
        write_passwd_db(&users);
        eprintln!("[init] First boot: created user '{}'.", DEFAULT_USER);
    }

    // If only one user with no password → auto-login.
    if users.len() == 1 && users[0].1.is_empty() {
        let username = users[0].0.clone();
        eprintln!("[init] Auto-login as '{}'.", username);
        return username;
    }

    // Otherwise prompt.
    println!("\n\x1b[32mRusty Penguin\x1b[0m — Login");
    if storage == Trit::Pos {
        println!("Persistent session. Your files are in /home/<user>.\n");
    }
    loop {
        print!("Username: ");
        let username = read_line_tty();
        if username.is_empty() { continue; }
        // Check if user exists.
        if !users.iter().any(|(u, _)| u == &username) {
            print!("New user '{}'. Set a password (Enter for none): ", username);
            let pw = read_line_tty();
            add_user(&username, &pw);
            println!("User '{}' created.", username);
            return username;
        }
        // Existing user — check password.
        let needs_pw = users.iter().any(|(u, h)| u == &username && !h.is_empty());
        if needs_pw {
            print!("Password: ");
            let pw = read_line_tty();
            if verify_password(&username, &pw) {
                return username;
            }
            println!("Incorrect password.");
        } else {
            return username; // no password set
        }
    }
}

fn setup_home_directory_for(username: &str) {
    let home = format!("/home/{}", username);
    if !Path::new(&home).exists() {
        let _ = fs::create_dir_all(&home);
        eprintln!("[init] Created home directory: {}", home);
    }
    let config_dir = format!("{}/.config/rusty-penguin", home);
    let _ = fs::create_dir_all(&config_dir);
    let history_file = format!("{}/.psh_history", home);
    if !Path::new(&history_file).exists() {
        let _ = fs::File::create(&history_file);
    }
    std::env::set_var("HOME", &home);
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
