# Building Rusty Penguin Packages

Simple guide to creating .rpkg packages for Rusty Penguin.

## Package Structure

Every .rpkg is a tar archive with this layout:

```
myapp-1.0.0/
├── manifest.toml          # Package metadata
├── bin/
│   └── myapp              # Executable (will be symlinked)
├── lib/
│   └── libmyapp.so        # Optional libraries
├── share/
│   └── myapp/
│       └── config.example # Optional config files
└── README.md              # Documentation
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

[libraries]
libmyapp = "lib/libmyapp.so"
```

## Creating a Package

### Step 1: Create directory structure

```bash
mkdir -p myapp-1.0.0/{bin,lib,share/myapp}
```

### Step 2: Add your files

```bash
# Copy your executable
cp /path/to/myapp myapp-1.0.0/bin/

# Copy libraries (if any)
cp /path/to/libmyapp.so myapp-1.0.0/lib/

# Copy config files
cp config.example myapp-1.0.0/share/myapp/
```

### Step 3: Create manifest.toml

```bash
cat > myapp-1.0.0/manifest.toml << 'EOF'
[package]
name = "myapp"
version = "1.0.0"
description = "My awesome application"
author = "Your Name"
license = "MIT"

[binaries]
myapp = "bin/myapp"
EOF
```

### Step 4: Add README

```bash
cat > myapp-1.0.0/README.md << 'EOF'
# MyApp v1.0.0

Description of your application.

## Installation

```
rpm install myapp-1.0.0.rpkg
```

## Usage

```
myapp [options]
```
EOF
```

### Step 5: Create tar archive

```bash
tar czf myapp-1.0.0.rpkg myapp-1.0.0/
```

### Step 6: Install and test

```bash
rpm install myapp-1.0.0.rpkg
rpm list
rpm info myapp
myapp --version  # Should work if it's in PATH
```

## Example: Hello World Package

```bash
# Create structure
mkdir -p hello-world-1.0.0/bin

# Create simple executable (shell script or compiled binary)
cat > hello-world-1.0.0/bin/hello << 'EOF'
#!/bin/sh
echo "Hello from Rusty Penguin!"
EOF
chmod +x hello-world-1.0.0/bin/hello

# Create manifest
cat > hello-world-1.0.0/manifest.toml << 'EOF'
[package]
name = "hello-world"
version = "1.0.0"
description = "Simple hello world program"
author = "Rusty Penguin Team"
license = "MIT"

[binaries]
hello = "bin/hello"
EOF

# Create package
tar czf hello-world-1.0.0.rpkg hello-world-1.0.0/

# Install
rpm install hello-world-1.0.0.rpkg

# Test
hello
```

## Built-in Packages (Phase 2)

Rusty Penguin will ship with pre-built packages:

1. **tis-stack-1.5.0** — Albert inference runtime
2. **dev-tools-1.0.0** — gcc, make, git
3. **text-tools-1.0.0** — sed, awk, grep enhancements
4. **net-tools-1.0.0** — curl, wget, netstat
5. **media-tools-1.0.0** — ffmpeg, imagemagick

## Notes

- Executables must be absolute (starting with `/`) or placed in `bin/`
- Libraries in `lib/` are extracted but not automatically linked
- `share/` contains config files, docs, data files
- Package names must be lowercase, alphanumeric + dash
- Use semantic versioning (MAJOR.MINOR.PATCH)
