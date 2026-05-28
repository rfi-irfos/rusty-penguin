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
//   @pkg <name> <version> <url> [dep ...]
// `rpm update <url>` caches it; `rpm install <name>` resolves the transitive
// dependency closure (topological order, cycle-detected) and installs each.

const REPO_CACHE: &str = "/opt/rusty-penguin/repo.tern";

#[derive(Clone, Debug, PartialEq)]
pub struct RepoPkg {
    pub name: String,
    pub version: String,
    pub url: String,
    pub deps: Vec<String>,
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
            let deps = it.map(|s| s.to_string()).collect();
            pkgs.push(RepoPkg { name, version, url, deps });
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

/// `rpm update <url>` — cache the repo index.
pub fn update_repo(url: &str) -> Result<String, String> {
    fs::create_dir_all(PKG_DIR).map_err(|e| e.to_string())?;
    let status = std::process::Command::new("/bin/busybox")
        .args(["wget", "-q", "-O", REPO_CACHE, url])
        .status()
        .map_err(|e| format!("could not run wget: {}", e))?;
    if !status.success() {
        return Err(format!("repo update failed: {}", url));
    }
    let text = fs::read_to_string(REPO_CACHE).map_err(|e| e.to_string())?;
    Ok(format!("repo updated: {} package(s) available", RepoIndex::parse(&text).pkgs.len()))
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
        log.push_str(&PackageManager::install(&pkg.url)?);
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
        @pkg app     1.0 http://r/app-1.0.rpkg lib gui\n\
        @pkg lib     2.1 http://r/lib-2.1.rpkg core\n\
        @pkg gui     0.9 http://r/gui-0.9.rpkg core\n\
        @pkg core    3.0 http://r/core-3.0.rpkg\n";

    fn none(_: &str) -> bool { false }

    #[test]
    fn test_index_parse() {
        let idx = RepoIndex::parse(SAMPLE);
        assert_eq!(idx.pkgs.len(), 4);
        let app = idx.get("app").unwrap();
        assert_eq!(app.version, "1.0");
        assert_eq!(app.url, "http://r/app-1.0.rpkg");
        assert_eq!(app.deps, vec!["lib".to_string(), "gui".to_string()]);
        assert!(idx.get("nope").is_none());
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
        let idx = RepoIndex::parse("@pkg a 1.0 http://r/a.rpkg ghost\n");
        assert!(idx.resolve("a", &none).unwrap_err().contains("not found"));
    }

    #[test]
    fn test_resolve_cycle() {
        let idx = RepoIndex::parse(
            "@pkg a 1 http://r/a.rpkg b\n@pkg b 1 http://r/b.rpkg a\n");
        assert!(idx.resolve("a", &none).unwrap_err().contains("cycle"));
    }
}
