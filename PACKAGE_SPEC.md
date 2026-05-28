# Rusty Penguin Package Format

Simple, zero-dependency package system for Rusty Penguin.

## Package Structure

A Rusty Penguin package (`.rpkg`) is a tar archive with:

```
myapp-1.0.0.rpkg
├── manifest.toml
├── bin/
│   ├── myapp
│   └── myapp-config
├── lib/
│   └── libmyapp.so
├── share/
│   └── myapp/
│       └── config.example
└── README.md
```

## manifest.toml Format

```toml
[package]
name = "myapp"
version = "1.0.0"
description = "My awesome application"
author = "Your Name"
license = "MIT"

[binaries]
myapp = "bin/myapp"
myapp-config = "bin/myapp-config"

[libraries]
libmyapp = "lib/libmyapp.so"

[dependencies]
# List runtime dependencies (other packages)
# These are loaded/mounted when myapp runs
```

## Installation

### Directory Structure

```
/opt/rusty-penguin/
├── packages/
│   ├── myapp-1.0.0/
│   │   ├── bin/
│   │   ├── lib/
│   │   └── manifest.toml
│   ├── tis-stack-1.5.0/
│   └── ...
└── bin/ → symlinks to latest versions
```

### Commands

```bash
# Install a package
rpm install myapp-1.0.0.rpkg

# List installed packages
rpm list

# Show package info
rpm info myapp

# Remove a package
rpm remove myapp

# Search for packages
rpm search keyword

# Update all packages
rpm update
```

## Repository Format

Central repository at `https://packages.rusty-penguin.dev/` (or local mirror):

```
/repository
├── packages/
│   ├── myapp-1.0.0.rpkg
│   ├── myapp-1.0.1.rpkg
│   ├── tis-stack-1.5.0.rpkg
│   └── ...
├── index.toml
└── signatures/ (future: GPG signatures)
```

`index.toml`:
```toml
[[packages]]
name = "myapp"
version = "1.0.0"
filename = "myapp-1.0.0.rpkg"
size = 1024000
sha256 = "abc123..."
dependencies = []

[[packages]]
name = "tis-stack"
version = "1.5.0"
filename = "tis-stack-1.5.0.rpkg"
size = 50000000
sha256 = "def456..."
dependencies = []
```

## Package Manager Implementation (Phase 2)

Roadmap:
1. **v1**: Extract tar, symlink binaries, update PATH
2. **v2**: Dependency resolution, install-time hooks
3. **v3**: GPG signatures, pinned versions, rollback
4. **v4**: Binary delta updates, incremental downloads

## Initial Package Set

Pre-built packages to ship:
1. `tis-stack-1.5.0` — Albert inference runtime, CLI tools
2. `dev-tools-1.0.0` — git, make, build essentials
3. `text-tools-1.0.0` — cat, grep, sed, awk enhancements
4. `net-tools-1.0.0` — curl, wget, iproute2
5. `media-tools-1.0.0` — ffmpeg, imagemagick

## Future: Building Packages

```bash
# Create package from source
rp-build -C ./myapp-src/
# Produces: myapp-1.0.0.rpkg

# Publish to repository
rp-publish myapp-1.0.0.rpkg --repo=https://packages.rusty-penguin.dev/
```
