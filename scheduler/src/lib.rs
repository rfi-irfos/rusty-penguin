use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use thiserror::Error;
use std::fs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TernaryState {
    Suppressed = -1,
    Dormant    = 0,
    Active     = 1,
}

impl TernaryState {
    pub fn symbol(self) -> &'static str {
        match self {
            TernaryState::Active     => "+1",
            TernaryState::Dormant    => " 0",
            TernaryState::Suppressed => "-1",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TernaryState::Active     => "ACTIVE",
            TernaryState::Dormant    => "DORMANT",
            TernaryState::Suppressed => "SUPPRESSED",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid:   i32,
    pub name:  String,
    pub state: TernaryState,
    pub vmrss_kb: u64,
}

#[derive(Error, Debug)]
pub enum SchedulerError {
    #[error("Process not found: {0}")]
    ProcessNotFound(i32),
    #[error("OS error: {0}")]
    OsError(String),
}

pub struct ProcessController;

impl ProcessController {
    /// Send a ternary state transition signal to a real Linux PID.
    pub fn transition(pid: i32, state: TernaryState) -> Result<(), SchedulerError> {
        let nix_pid = Pid::from_raw(pid);
        let signal = match state {
            TernaryState::Active     => Signal::SIGCONT,
            TernaryState::Dormant    => Signal::SIGSTOP,
            TernaryState::Suppressed => Signal::SIGTERM,
        };
        signal::kill(nix_pid, signal).map_err(|e| {
            if e == nix::errno::Errno::ESRCH {
                SchedulerError::ProcessNotFound(pid)
            } else {
                SchedulerError::OsError(e.to_string())
            }
        })
    }

    /// Read real processes from /proc and annotate each with a TernaryState.
    /// Heuristic: running/sleeping high-mem = Active; idle/stopped = Dormant;
    /// zombie/dead = Suppressed.
    pub fn list_processes() -> Vec<ProcessInfo> {
        let mut procs = Vec::new();

        let Ok(dir) = fs::read_dir("/proc") else { return procs };

        for entry in dir.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            let Ok(pid) = name_str.parse::<i32>() else { continue };

            let status_path = format!("/proc/{}/status", pid);
            let Ok(status) = fs::read_to_string(&status_path) else { continue };

            let proc_name = parse_field(&status, "Name").unwrap_or_else(|| "?".into());
            let proc_state_char = parse_field(&status, "State")
                .and_then(|s| s.chars().next())
                .unwrap_or('?');
            let vmrss_kb: u64 = parse_field(&status, "VmRSS")
                .and_then(|s| s.split_whitespace().next()?.parse().ok())
                .unwrap_or(0);

            let state = infer_trit_state(proc_state_char, vmrss_kb);
            procs.push(ProcessInfo { pid, name: proc_name, state, vmrss_kb });
        }

        procs.sort_by_key(|p| p.pid);
        procs
    }
}

/// Map Linux process state character to ternary state.
fn infer_trit_state(state_char: char, vmrss_kb: u64) -> TernaryState {
    match state_char {
        'R' => TernaryState::Active,                          // Running
        'S' if vmrss_kb > 1024 => TernaryState::Active,      // Sleeping but resident
        'S' | 'I' => TernaryState::Dormant,                  // Idle / interruptible sleep
        'T' | 't' => TernaryState::Dormant,                  // Stopped (SIGSTOP)
        'Z' | 'X' => TernaryState::Suppressed,               // Zombie / dead
        'D' => TernaryState::Active,                          // Uninterruptible IO
        _ => TernaryState::Dormant,
    }
}

fn parse_field(status: &str, field: &str) -> Option<String> {
    status.lines()
        .find(|l| l.starts_with(field))
        .and_then(|l| l.splitn(2, ':').nth(1))
        .map(|v| v.trim().to_string())
}
