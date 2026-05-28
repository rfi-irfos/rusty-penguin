# Rusty Penguin Distribution Layer

Build plan for making Rusty Penguin a complete, daily-usable operating system distribution.

## Current State (2026-05-28)

**What Works:**
- ✅ Linux kernel boot (6.17) + Rust init (PID 1)
- ✅ Graphical desktop with window manager
- ✅ Shell (psh) with ~90 commands
- ✅ Text editor (graphical)
- ✅ Bare-metal kernel option (optional)

**What's Missing (Blocking Daily Use):**
- ❌ File manager that works with real filesystems (currently in-memory only)
- ❌ Package manager for installing software
- ❌ System settings/configuration UI
- ❌ Persistent home directory setup
- ❌ Service management (systemd alternative or custom)
- ❌ TIS integration for inference

## Distribution Layer Components (Priority Order)

### Phase 1: File System Usability (Week 1)
**Goal: Users can browse /home and work with real files**

- [ ] **FileManager.rs v2**: Replace in-memory VFS with real filesystem access
  - [ ] Directory browsing using std::fs
  - [ ] File listing with metadata (size, permissions, date)
  - [ ] Context menu for open/edit/delete
  - [ ] Current directory display in title bar
  - [ ] Breadcrumb navigation
  
- [ ] **Home directory setup**:
  - [ ] Create /home/rusty-penguin on first boot
  - [ ] Set HOME environment variable
  - [ ] Persistent user shell history
  - [ ] Local configuration directory (~/.config/rusty-penguin)

### Phase 2: Package Management (Week 2) — IN PROGRESS
**Goal: Users can `install` software**

- [x] **Shell integration**: `rpm` command in psh
- [ ] **Package system**:
  - [ ] Simple repository format (tarball + manifest)
  - [ ] `rpm install <package-name>` command
  - [ ] Install to /opt/rusty-penguin-packages/<name>/
  - [ ] PATH integration so installed binaries work
  - [ ] Dependency resolution (basic)

- [ ] **Included packages**:
  - [ ] ffmpeg (for inference demos)
  - [ ] git (for development)
  - [ ] curl (for network testing)
  - [ ] TIS stack binaries

### Phase 3: System Configuration (Week 3)
**Goal: Settings UI, persistent preferences**

- [ ] **Settings application**:
  - [ ] Desktop background selection
  - [ ] Taskbar position (top/bottom/left/right)
  - [ ] Keyboard repeat rate
  - [ ] Color theme (dark/light)
  - [ ] Window manager behavior (maximize/snap)

- [ ] **Config persistence**:
  - [ ] ~/.config/rusty-penguin/settings.toml
  - [ ] Load on init, apply to desktop/shell
  - [ ] UI to edit without text editor

### Phase 4: TIS Integration (Week 4)
**Goal: Run inference from the desktop**

- [ ] **AI Runtime launcher**:
  - [ ] Desktop icon for TIS console
  - [ ] Model selector
  - [ ] Inference prompt UI
  - [ ] Result display

- [ ] **Environment**:
  - [ ] Deploy albert. binary
  - [ ] Model directory setup
  - [ ] PYTHONPATH / library integration

### Phase 5: Service Management (Ongoing)
**Goal: Proper boot sequence, logging, service control**

- [ ] **Init system**:
  - [ ] Service descriptor format (.service-like)
  - [ ] Enable/disable services
  - [ ] Logging to /var/log/
  - [ ] Boot sequence control

- [ ] **System services**:
  - [ ] SSH server (optional, install-time choice)
  - [ ] Cron for scheduled tasks
  - [ ] D-Bus for IPC

## Implementation Approach

### Track 1: Linux Userspace (Production Path)
- Work directly on Linux host + ISO
- Use std::fs and libc for features
- Fast iteration, immediate usability
- Goal: Swap from Ubuntu by end of June

### Track 2: Bare-Metal (Pure Rust, Parallel Effort)
- Backport architecture from Linux track
- Build custom syscalls for filesystem access
- Schedule for Phase 2 after Linux stabilizes
- Goal: Parity with Linux track by Q3

## Metrics for "Daily-Drivable"

- [ ] Boot completes in <5 seconds
- [ ] Can navigate home directory with file manager
- [ ] Can install a package and run it
- [ ] Can save files and they persist across reboot
- [ ] Desktop is responsive (no lag, smooth rendering)
- [ ] Can run TIS inference from desktop
- [ ] Keyboard + mouse input feel responsive
- [ ] No crashes on normal workflows

## Related Epics

- **File System Persistence**: Implement real filesystem abstraction
- **Package Ecosystem**: Build and curate basic package repository
- **User Experience**: Desktop polish, responsiveness, visual polish
- **Hardware Support**: Expand beyond framebuffer + keyboard to audio, USB, network
