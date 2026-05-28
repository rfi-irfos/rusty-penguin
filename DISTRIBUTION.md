# Rusty Penguin Distribution Layer

Build plan for making Rusty Penguin a complete, daily-usable operating system distribution.

## Current State (2026-05-28 Session 2)

**What Works:**
- ✅ Linux kernel boot (6.17) + Rust init (PID 1)
- ✅ Graphical desktop with window manager (stable multi-window, drag/resize/minimize)
- ✅ Shell (psh) with ~90 commands
- ✅ Text editor (graphical)
- ✅ Bare-metal kernel option (full Rust kernel, 64-bit long mode)
- ✅ FileManager with real filesystem browsing (syscall 14: sys_listdir)
- ✅ Settings application (theme, window snap, taskbar, auto-save)
- ✅ TIS Console (ternary arithmetic: trit, mul, div)
- ✅ Process Monitor (process viewer)
- ✅ System Info (OS/kernel/memory display)
- ✅ Package manager (rpm: install .rpkg packages to /opt/rusty-penguin/)

**What's Missing (Blocking Daily Use):**
- ⚠️ Persistent settings (UI built, needs load/save to ~/.config/rusty-penguin/settings.toml)
- ⚠️ TIS integration for real inference (UI built, needs albert. binary integration)
- ❌ Persistent home directory setup
- ❌ Service management (systemd alternative or custom)
- ❌ File operation syscalls (copy, delete, rename)
- ❌ Real filesystem on bare-metal (currently ramfs only)

## Distribution Layer Components (Priority Order)

### Phase 1: File System Usability (Week 1) — COMPLETE
**Goal: Users can browse /home and work with real files**

- [x] **FileManager.rs v2 Complete**: Real filesystem access via syscall 14 (sys_listdir)
  - [x] Directory browsing with Up/Down/Enter/Backspace navigation
  - [x] File listing with metadata (size, display)
  - [x] Current directory display in title bar
  - [x] Clipboard copy (C key)
  - [ ] Delete (D key - framework ready, needs kernel syscall)
  - [ ] Rename (future enhancement)
  
- [ ] **Home directory setup**:
  - [ ] Create /home/rusty-penguin on first boot
  - [ ] Set HOME environment variable
  - [ ] Persistent user shell history
  - [ ] Local configuration directory (~/.config/rusty-penguin)

### Phase 2: Package Management (Week 2) — COMPLETE
**Goal: Users can `install` software**

- [x] **Shell integration**: `rpm` command in psh
- [x] **Package system**:
  - [x] Simple tar-based format (.rpkg = tar.gz)
  - [x] `rpm install <package.rpkg>` command with tar extraction
  - [x] Install to /opt/rusty-penguin/packages/<name>/
  - [x] Automatic symlinks in /opt/rusty-penguin/bin/ for binaries
  - [x] PATH integration ready (just add to shell PATH)
  - [ ] Dependency resolution (future enhancement)

- [ ] **Included packages**:
  - [ ] ffmpeg (for inference demos)
  - [ ] git (for development)
  - [ ] curl (for network testing)
  - [ ] TIS stack binaries

### Phase 3: System Configuration (Week 3) — IN PROGRESS
**Goal: Settings UI, persistent preferences**

- [x] **Settings application**: Keyboard-navigable preferences UI
  - [x] Color theme (dark/light) - UI ready
  - [x] Window manager snap behavior - UI ready
  - [x] Taskbar positioning - UI ready
  - [x] Auto-save toggle - UI ready
  - [ ] Desktop background selection (future)
  - [ ] Keyboard repeat rate (future)

- [ ] **Config persistence**:
  - [ ] ~/.config/rusty-penguin/settings.toml (framework ready)
  - [ ] Load on init, apply to desktop/shell
  - [ ] Write changes on toggle

### Phase 4: TIS Integration (Week 4) — IN PROGRESS
**Goal: Run inference from the desktop**

- [x] **AI Runtime launcher**: TIS Console application
  - [x] Desktop icon for TIS console
  - [x] Ternary arithmetic commands (trit, mul, div)
  - [x] Text input buffer with command parsing
  - [x] Scrollable output history
  - [ ] Real albert. binary integration (needs env setup)
  - [ ] Model selector (future)

- [ ] **Environment**:
  - [ ] Deploy albert. binary to /opt/rusty-penguin/
  - [ ] Model directory setup
  - [ ] PATH integration for command execution

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

## Session 2 Progress (2026-05-28)

### Completed
- Enhanced Settings app with live state tracking (theme, window snap, taskbar, auto-save)
- Settings now toggle dynamically with ENTER key (was hardcoded before)
- Added sys_delete syscall (15) for file operations in kernel
- FileManager D key now calls actual sys_delete (was placeholder)
- Created persistence framework for settings (load_from_disk/save_to_disk)
- Settings format designed: theme=dark, window_snap=true, etc (key=value)

### Architecture Ready
- Settings persistence framework shows expected config file structure
- On Linux track: will save to ~/.config/rusty-penguin/settings.ini (real filesystem)
- On bare-metal: ephemeral in ramfs (can extend with persistent storage)
- File operation syscalls: delete ready, copy/rename framework for future

### Test Results
- 7 applications stable on desktop (Term, Files, Edit, Procs, Cal, Prefs, TIS)
- Multi-window rendering stable (no flickering observed)
- File navigation with syscall 14 (sys_listdir) working correctly
- Settings UI shows current values dynamically

### Next Priority
- Implement full file I/O syscalls (open with flags, write to disk)
- Real TIS inference integration with albert. binary
- Home directory setup on first boot
- Service/init system for boot sequence

## Related Epics

- **File System Persistence**: Implement real filesystem abstraction
- **Package Ecosystem**: Build and curate basic package repository
- **User Experience**: Desktop polish, responsiveness, visual polish
- **Hardware Support**: Expand beyond framebuffer + keyboard to audio, USB, network
