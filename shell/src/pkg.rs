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
            let pkg_path = args.get(1)
                .ok_or("Usage: rpm install <package.rpkg | http(s)://…/package.rpkg>")?;
            PackageManager::install(pkg_path)
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
            Err("Usage: rpm [install|list|info|remove|search] [args...]\n\
                 install accepts a local .rpkg or an http(s) URL".to_string())
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
}
