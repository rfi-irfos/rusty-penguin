// Package manager for Rusty Penguin
// Handles .rpkg installation, listing, removal

use std::fs;
use std::path::{Path, PathBuf};

const PKG_DIR: &str = "/opt/rusty-penguin/packages";
const BIN_DIR: &str = "/opt/rusty-penguin/bin";

pub struct PackageManager;

impl PackageManager {
    pub fn install(pkg_path: &str) -> Result<String, String> {
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
        fs::create_dir_all(&pkg_install_dir).map_err(|e| e.to_string())?;

        // Copy the .rpkg file to packages directory
        let dest = pkg_install_dir.join("archive.rpkg");
        fs::copy(pkg_path, &dest).map_err(|e| e.to_string())?;

        // For now, just copy the file. Later: extract tar, symlink binaries
        // TODO: Extract tar contents, parse manifest.toml, create symlinks in BIN_DIR

        Ok(format!("Installed package: {}", pkg_name))
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
            let pkg_path = args.get(1).ok_or("Usage: rpm install <package.rpkg>")?;
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
            Err("Usage: rpm [install|list|info|remove|search] [args...]".to_string())
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
}
