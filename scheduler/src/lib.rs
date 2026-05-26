use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TernaryState {
    Suppressed = -1,
    Dormant = 0,
    Active = 1,
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
    pub fn transition(pid: i32, state: TernaryState) -> Result<(), SchedulerError> {
        let nix_pid = Pid::from_raw(pid);
        
        let signal = match state {
            TernaryState::Active => Signal::SIGCONT,
            TernaryState::Dormant => Signal::SIGSTOP,
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
}
