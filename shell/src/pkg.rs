// Package manager for Rusty Penguin
// Handles .rpkg installation, listing, removal

use std::fs;
use std::path::{Path, PathBuf};
use tar::Archive;
use std::os::unix::fs as unix_fs;

const PKG_DIR: &str = "/opt/rusty-penguin/packages";
const BIN_DIR: &str = "/opt/rusty-penguin/bin";

/// A package source is one of: a local `.rpkg` path, or an `http(s)` URL.
pub fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

/// Fetch a remote `.rpkg` to /tmp using the bundled static busybox wget, so the
/// rest of install() works on a local file. Networking is brought up by init.
fn fetch_remote(url: &str) -> Result<String, String> {
    let name = url.rsplit('/').next().filter(|s| !s.is_empty())
        .ok_or("Could not derive filename from URL")?;
    if !name.ends_with(".rpkg") {
        return Err("URL must point to a .rpkg file".to_string());
    }
    let dest = format!("/tmp/{}", name);
    let status = std::process::Command::new("/bin/busybox")
        .args(["wget", "-q", "-O", &dest, url])
        .status()
        .map_err(|e| format!("could not run wget: {}", e))?;
    if !status.success() {
        let _ = fs::remove_file(&dest);
        return Err(format!("download failed: {}", url));
    }
    Ok(dest)
}

// ── Package repository + dependency resolution ───────────────────────────────
// The repo index is a `.tern` document (ternary-native format):
//   @pkg <name> <version> <url> <sha256|-> [dep ...]
// `rpm update <url>` caches it; `rpm install <name>` resolves the transitive
// dependency closure (topological order, cycle-detected), downloads each, and
// verifies its SHA-256 against the index before installing (integrity check).

const REPO_CACHE: &str = "/opt/rusty-penguin/repo.tern";

#[derive(Clone, Debug, PartialEq)]
pub struct RepoPkg {
    pub name: String,
    pub version: String,
    pub url: String,
    pub sha256: Option<String>,   // expected hex digest; None when index says "-"
    pub deps: Vec<String>,
}

/// SHA-256 of a file, lowercase hex. Used to verify a downloaded package matches
/// the digest the repo index published (tamper/corruption detection).
fn file_sha256(path: &str) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let bytes = fs::read(path).map_err(|e| format!("read {}: {}", path, e))?;
    let digest = Sha256::digest(&bytes);
    let mut s = String::with_capacity(64);
    for b in digest { s.push_str(&format!("{:02x}", b)); }
    Ok(s)
}

pub struct RepoIndex {
    pub pkgs: Vec<RepoPkg>,
}

impl RepoIndex {
    /// Parse a `.tern` repo index. `#` comments and blank lines ignored.
    pub fn parse(text: &str) -> Self {
        let mut pkgs = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            let mut it = line.split_whitespace();
            if it.next() != Some("@pkg") { continue; }
            let name = match it.next() { Some(n) => n.to_string(), None => continue };
            let version = it.next().unwrap_or("0").to_string();
            let url = match it.next() { Some(u) => u.to_string(), None => continue };
            let sha256 = it.next().filter(|s| *s != "-").map(|s| s.to_string());
            let deps = it.map(|s| s.to_string()).collect();
            pkgs.push(RepoPkg { name, version, url, sha256, deps });
        }
        RepoIndex { pkgs }
    }

    pub fn get(&self, name: &str) -> Option<&RepoPkg> {
        self.pkgs.iter().find(|p| p.name == name)
    }

    /// Resolve the install order for `name` + transitive deps, skipping anything
    /// already installed. Dependency-first (topological) order; errors on a
    /// missing package or a dependency cycle.
    pub fn resolve(&self, name: &str, is_installed: &dyn Fn(&str) -> bool)
        -> Result<Vec<String>, String>
    {
        let mut order = Vec::new();
        let mut visiting: Vec<String> = Vec::new();
        self.visit(name, is_installed, &mut order, &mut visiting)?;
        Ok(order)
    }

    fn visit(&self, name: &str, is_installed: &dyn Fn(&str) -> bool,
             order: &mut Vec<String>, visiting: &mut Vec<String>) -> Result<(), String> {
        if order.iter().any(|n| n == name) { return Ok(()); }   // already resolved
        if is_installed(name) { return Ok(()); }                // present — skip
        if visiting.iter().any(|n| n == name) {
            return Err(format!("dependency cycle involving '{}'", name));
        }
        let pkg = self.get(name)
            .ok_or_else(|| format!("package not found in repo: '{}'", name))?;
        visiting.push(name.to_string());
        for dep in &pkg.deps {
            self.visit(dep, is_installed, order, visiting)?;
        }
        visiting.pop();
        order.push(name.to_string());
        Ok(())
    }
}

/// Is a package already installed (dir `<name>` or `<name>-<version>` in PKG_DIR)?
fn is_installed(name: &str) -> bool {
    if let Ok(rd) = fs::read_dir(PKG_DIR) {
        let prefix = format!("{}-", name);
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n == name || n.starts_with(&prefix) { return true; }
        }
    }
    false
}

/// Provisioned repo public key (raw 32-byte ed25519). Present → signed mode:
/// the index must carry a valid signature. Absent → unsigned (with a warning),
/// like apt with no keyring. The matching private key is held offline by the
/// repo publisher and never ships in the OS.
const REPO_PUBKEY_PATH: &str = "/opt/rusty-penguin/repo.pub";

/// Verify a raw ed25519 signature (`sig`, 64 bytes) of `msg` under `pubkey`
/// (32 bytes). The trust root for package authenticity.
fn verify_sig(msg: &[u8], sig: &[u8], pubkey: &[u8]) -> Result<(), String> {
    use ed25519_dalek::{VerifyingKey, Signature};
    let pk: [u8; 32] = pubkey.try_into().map_err(|_| "repo key must be 32 bytes".to_string())?;
    let vk = VerifyingKey::from_bytes(&pk).map_err(|e| format!("bad repo key: {}", e))?;
    let s: [u8; 64] = sig.try_into().map_err(|_| "signature must be 64 bytes".to_string())?;
    vk.verify_strict(msg, &Signature::from_bytes(&s))
        .map_err(|_| "repo signature verification FAILED".to_string())
}

/// `rpm update <url>` — cache the repo index, verifying its authenticity when a
/// repo key is provisioned.
pub fn update_repo(url: &str) -> Result<String, String> {
    fs::create_dir_all(PKG_DIR).map_err(|e| e.to_string())?;
    let status = std::process::Command::new("/bin/busybox")
        .args(["wget", "-q", "-O", REPO_CACHE, url])
        .status()
        .map_err(|e| format!("could not run wget: {}", e))?;
    if !status.success() {
        return Err(format!("repo update failed: {}", url));
    }
    let index_bytes = fs::read(REPO_CACHE).map_err(|e| e.to_string())?;

    // Authenticity: if a repo key is provisioned, require a valid index signature.
    let note = if Path::new(REPO_PUBKEY_PATH).exists() {
        let pubkey = fs::read(REPO_PUBKEY_PATH).map_err(|e| e.to_string())?;
        let sig_tmp = "/tmp/repo.tern.sig";
        let ok = std::process::Command::new("/bin/busybox")
            .args(["wget", "-q", "-O", sig_tmp, &format!("{}.sig", url)])
            .status().map(|s| s.success()).unwrap_or(false);
        if !ok {
            let _ = fs::remove_file(REPO_CACHE);
            return Err("repo is in signed mode but no .sig was available".to_string());
        }
        let sig = fs::read(sig_tmp).map_err(|e| e.to_string())?;
        if let Err(e) = verify_sig(&index_bytes, &sig, &pubkey) {
            let _ = fs::remove_file(REPO_CACHE);
            return Err(e);
        }
        " (signature verified)"
    } else {
        " (UNVERIFIED — provision /opt/rusty-penguin/repo.pub for authenticity)"
    };

    let n = RepoIndex::parse(&String::from_utf8_lossy(&index_bytes)).pkgs.len();
    Ok(format!("repo updated: {} package(s){}", n, note))
}

/// `rpm install <name>` — resolve deps from the cached repo and install all.
pub fn install_by_name(name: &str) -> Result<String, String> {
    let text = fs::read_to_string(REPO_CACHE)
        .map_err(|_| "no repo index — run `rpm update <url>` first".to_string())?;
    let idx = RepoIndex::parse(&text);
    let order = idx.resolve(name, &|n| is_installed(n))?;
    if order.is_empty() {
        return Ok(format!("{} is already installed", name));
    }
    let mut log = String::new();
    for n in &order {
        let pkg = idx.get(n).ok_or_else(|| format!("package not found: {}", n))?;
        // Download, verify integrity against the index, then install locally.
        let local = fetch_remote(&pkg.url)?;
        if let Some(want) = &pkg.sha256 {
            let got = file_sha256(&local)?;
            if !got.eq_ignore_ascii_case(want) {
                let _ = fs::remove_file(&local);
                return Err(format!(
                    "integrity check FAILED for '{}': expected {}, got {}", n, want, got));
            }
            log.push_str(&format!("  {} sha256 ok\n", n));
        }
        log.push_str(&PackageManager::install(&local)?);
        log.push('\n');
    }
    Ok(format!("installed {} (+{} dep(s)):\n{}", name, order.len().saturating_sub(1), log))
}

pub struct PackageManager;

impl PackageManager {
    pub fn install(pkg_path: &str) -> Result<String, String> {
        // Remote source? Fetch it first, then install from the local copy.
        let local;
        let pkg_path: &str = if is_url(pkg_path) {
            local = fetch_remote(pkg_path)?;
            &local
        } else {
            pkg_path
        };

        // Check if file exists
        if !Path::new(pkg_path).exists() {
            return Err(format!("Package file not found: {}", pkg_path));
        }

        // Ensure package directory exists
        fs::create_dir_all(PKG_DIR).map_err(|e| e.to_string())?;

        // Extract package name and version from filename
        let filename = Path::new(pkg_path)
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("Invalid package filename")?;

        let pkg_name = filename
            .strip_suffix(".rpkg")
            .ok_or("Package must end with .rpkg")?;

        // Create package directory
        let pkg_install_dir = PathBuf::from(PKG_DIR).join(pkg_name);
        if pkg_install_dir.exists() {
            return Err(format!("Package already installed: {}", pkg_name));
        }
        fs::create_dir_all(&pkg_install_dir).map_err(|e| e.to_string())?;

        // Extract tar archive
        let file = fs::File::open(pkg_path).map_err(|e| e.to_string())?;
        let mut archive = Archive::new(file);

        archive
            .unpack(&pkg_install_dir)
            .map_err(|e| format!("Failed to extract package: {}", e))?;

        // Create symlinks for binaries (if manifest exists)
        fs::create_dir_all(BIN_DIR).map_err(|e| e.to_string())?;
        let bin_src_dir = pkg_install_dir.join("bin");
        if bin_src_dir.exists() {
            if let Ok(entries) = fs::read_dir(&bin_src_dir) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                            let link_path = PathBuf::from(BIN_DIR).join(filename);
                            // Remove old symlink if it exists
                            let _ = fs::remove_file(&link_path);
                            // Create symlink
                            let _ = unix_fs::symlink(&path, &link_path);
                        }
                    }
                }
            }
        }

        Ok(format!("Installed package: {} (extracted and linked)", pkg_name))
    }

    pub fn list() -> Result<String, String> {
        if !Path::new(PKG_DIR).exists() {
            return Ok("No packages installed.".to_string());
        }

        let mut output = String::from("Installed packages:\n");
        let entries = fs::read_dir(PKG_DIR).map_err(|e| e.to_string())?;

        let mut packages: Vec<String> = entries
            .filter_map(|e| {
                e.ok().and_then(|entry| {
                    entry.file_name().into_string().ok()
                })
            })
            .collect();

        packages.sort();

        if packages.is_empty() {
            return Ok("No packages installed.".to_string());
        }

        for pkg in packages {
            output.push_str(&format!("  {}\n", pkg));
        }

        Ok(output)
    }

    pub fn info(pkg_name: &str) -> Result<String, String> {
        let pkg_path = PathBuf::from(PKG_DIR).join(pkg_name);

        if !pkg_path.exists() {
            return Err(format!("Package not found: {}", pkg_name));
        }

        let mut output = String::from("Package: ");
        output.push_str(pkg_name);
        output.push('\n');

        // Check for manifest.toml
        let manifest_path = pkg_path.join("manifest.toml");
        if manifest_path.exists() {
            if let Ok(manifest) = fs::read_to_string(&manifest_path) {
                output.push_str(&manifest);
            }
        } else {
            output.push_str("  Status: Installed (no manifest found)\n");
        }

        Ok(output)
    }

    pub fn remove(pkg_name: &str) -> Result<String, String> {
        let pkg_path = PathBuf::from(PKG_DIR).join(pkg_name);

        if !pkg_path.exists() {
            return Err(format!("Package not found: {}", pkg_name));
        }

        // Remove package directory
        fs::remove_dir_all(&pkg_path).map_err(|e| e.to_string())?;

        Ok(format!("Removed package: {}", pkg_name))
    }

    pub fn search(_query: &str) -> Result<String, String> {
        // Placeholder for package search
        // Later: will query remote repository
        Ok("Package search coming in phase 2.\n".to_string())
    }
}

pub fn cmd_rpm(args: &[&str]) -> Result<String, String> {
    match args.get(0).map(|s| *s) {
        Some("install") => {
            let arg = args.get(1)
                .ok_or("Usage: rpm install <name | package.rpkg | http(s)://…/package.rpkg>")?;
            // A path/URL/.rpkg installs directly; a bare name resolves from the
            // repo index (with its dependencies).
            if is_url(arg) || arg.ends_with(".rpkg") || Path::new(arg).exists() {
                PackageManager::install(arg)
            } else {
                install_by_name(arg)
            }
        }
        Some("update") => {
            let url = args.get(1).ok_or("Usage: rpm update <index-url>")?;
            update_repo(url)
        }
        Some("list") => PackageManager::list(),
        Some("info") => {
            let pkg_name = args.get(1).ok_or("Usage: rpm info <package>")?;
            PackageManager::info(pkg_name)
        }
        Some("remove") | Some("uninstall") => {
            let pkg_name = args.get(1).ok_or("Usage: rpm remove <package>")?;
            PackageManager::remove(pkg_name)
        }
        Some("search") => {
            let query = args.get(1).ok_or("Usage: rpm search <query>")?;
            PackageManager::search(query)
        }
        _ => {
            Err("Usage: rpm [install|update|list|info|remove|search] [args...]\n\
                 install accepts a repo name, a local .rpkg, or an http(s) URL;\n\
                 update <url> refreshes the package index".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpm_help() {
        let result = cmd_rpm(&[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Usage"));
    }

    #[test]
    fn test_is_url() {
        assert!(is_url("http://repo.rusty/foo.rpkg"));
        assert!(is_url("https://repo.rusty/foo.rpkg"));
        assert!(!is_url("/tmp/foo.rpkg"));
        assert!(!is_url("foo.rpkg"));
        assert!(!is_url("ftp://x/foo.rpkg"));
    }

    const SAMPLE: &str = "\
        # rusty-penguin repo index (.tern)\n\
        @pkg app     1.0 http://r/app-1.0.rpkg - lib gui\n\
        @pkg lib     2.1 http://r/lib-2.1.rpkg - core\n\
        @pkg gui     0.9 http://r/gui-0.9.rpkg - core\n\
        @pkg core    3.0 http://r/core-3.0.rpkg -\n";

    fn none(_: &str) -> bool { false }

    #[test]
    fn test_index_parse() {
        let idx = RepoIndex::parse(SAMPLE);
        assert_eq!(idx.pkgs.len(), 4);
        let app = idx.get("app").unwrap();
        assert_eq!(app.version, "1.0");
        assert_eq!(app.url, "http://r/app-1.0.rpkg");
        assert_eq!(app.sha256, None); // "-" means no digest
        assert_eq!(app.deps, vec!["lib".to_string(), "gui".to_string()]);
        assert!(idx.get("nope").is_none());
    }

    #[test]
    fn test_index_parse_with_sha() {
        let idx = RepoIndex::parse(
            "@pkg x 1 http://r/x.rpkg abc123DEF lib\n");
        let x = idx.get("x").unwrap();
        assert_eq!(x.sha256.as_deref(), Some("abc123DEF"));
        assert_eq!(x.deps, vec!["lib".to_string()]);
    }

    #[test]
    fn test_verify_sig() {
        use ed25519_dalek::{Signer, SigningKey};
        let sk = SigningKey::from_bytes(&[7u8; 32]);   // fixed seed → deterministic
        let vk = sk.verifying_key();
        let msg = b"@pkg core 3.0 http://r/core.rpkg -\n";
        let sig = sk.sign(msg);
        // Correct signature verifies; tampered message / wrong key fail.
        assert!(verify_sig(msg, &sig.to_bytes(), vk.as_bytes()).is_ok());
        assert!(verify_sig(b"tampered", &sig.to_bytes(), vk.as_bytes()).is_err());
        let other = SigningKey::from_bytes(&[9u8; 32]).verifying_key();
        assert!(verify_sig(msg, &sig.to_bytes(), other.as_bytes()).is_err());
        // Malformed inputs are rejected, not panicked.
        assert!(verify_sig(msg, &[0u8; 10], vk.as_bytes()).is_err());
        assert!(verify_sig(msg, &sig.to_bytes(), &[0u8; 5]).is_err());
    }

    #[test]
    fn test_file_sha256() {
        // Known vector: SHA-256("abc")
        let p = std::env::temp_dir().join("rp_pkg_sha_test.bin");
        std::fs::write(&p, b"abc").unwrap();
        let got = file_sha256(p.to_str().unwrap()).unwrap();
        assert_eq!(got, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn test_resolve_topological() {
        let idx = RepoIndex::parse(SAMPLE);
        let order = idx.resolve("app", &none).unwrap();
        // deps must precede dependents
        let pos = |n: &str| order.iter().position(|x| x == n).unwrap();
        assert!(pos("core") < pos("lib"));
        assert!(pos("core") < pos("gui"));
        assert!(pos("lib") < pos("app"));
        assert!(pos("gui") < pos("app"));
        assert_eq!(*order.last().unwrap(), "app");
        // core appears exactly once despite being a shared dep
        assert_eq!(order.iter().filter(|n| *n == "core").count(), 1);
    }

    #[test]
    fn test_resolve_skips_installed() {
        let idx = RepoIndex::parse(SAMPLE);
        let order = idx.resolve("app", &|n| n == "lib" || n == "core").unwrap();
        assert!(!order.contains(&"lib".to_string()));
        assert!(!order.contains(&"core".to_string()));
        assert!(order.contains(&"gui".to_string()));
        assert!(order.contains(&"app".to_string()));
    }

    #[test]
    fn test_resolve_missing_dep() {
        let idx = RepoIndex::parse("@pkg a 1.0 http://r/a.rpkg - ghost\n");
        assert!(idx.resolve("a", &none).unwrap_err().contains("not found"));
    }

    #[test]
    fn test_resolve_cycle() {
        let idx = RepoIndex::parse(
            "@pkg a 1 http://r/a.rpkg - b\n@pkg b 1 http://r/b.rpkg - a\n");
        assert!(idx.resolve("a", &none).unwrap_err().contains("cycle"));
    }
}
