//! Child process management.

use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};

use crate::config::ProcessConfig;
use crate::error::Result;

/// Child process state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProcessState {
    /// Process is not running.
    Stopped,
    /// Process is starting.
    Starting,
    /// Process is running.
    Running,
    /// Process is stopping.
    Stopping,
    /// Process exited with an error.
    Failed,
}

/// Child process manager.
pub struct ChildProcess {
    /// Process configuration.
    config: ProcessConfig,

    /// Process ID.
    pid: Option<u32>,

    /// Tokio child handle (kept alive for wait/kill).
    child: Option<tokio::process::Child>,

    /// Process state.
    state: ProcessState,
}

impl ChildProcess {
    /// Create a new child process manager.
    pub fn new(config: ProcessConfig) -> Self {
        Self {
            config,
            pid: None,
            child: None,
            state: ProcessState::Stopped,
        }
    }

    /// Start the child process.
    pub async fn start(&mut self) -> Result<()> {
        tracing::info!("Starting child process: {}", self.config.command);

        let mut cmd = tokio::process::Command::new(&self.config.command);
        cmd.args(&self.config.args);

        if let Some(dir) = &self.config.working_dir {
            cmd.current_dir(dir);
        }

        let child = cmd.spawn()?;
        let pid = child.id().unwrap_or(0);

        if pid == 0 {
            return Err(crate::error::Error::Process(
                "child process has no valid PID".to_string(),
            ));
        }

        self.pid = Some(pid);
        self.child = Some(child);
        self.state = ProcessState::Running;

        tracing::info!("Child process started with PID {}", pid);
        Ok(())
    }

    /// Stop the child process gracefully.
    pub async fn stop(&mut self) -> Result<()> {
        if let Some(pid) = self.pid {
            tracing::info!("Stopping child process PID {}", pid);

            // Send SIGTERM
            self.state = ProcessState::Stopping;
            if let Err(e) = signal::kill(Pid::from_raw(pid as i32), Signal::SIGTERM) {
                tracing::warn!("failed to send SIGTERM to PID {}: {}", pid, e);
            }

            // Wait for graceful shutdown using the child handle
            let timeout = std::time::Duration::from_secs(self.config.shutdown_timeout_secs);
            if let Some(ref mut child) = self.child {
                let _ = tokio::time::timeout(timeout, child.wait()).await;
            }

            // Force kill if still running
            if self.is_running() {
                tracing::warn!(
                    "Child process PID {} did not exit gracefully, sending SIGKILL",
                    pid
                );
                if let Err(e) = signal::kill(Pid::from_raw(pid as i32), Signal::SIGKILL) {
                    tracing::warn!("failed to send SIGKILL to PID {}: {}", pid, e);
                }
            }

            // Take the child handle so it's dropped
            self.child = None;
            self.pid = None;
            self.state = ProcessState::Stopped;
            tracing::info!("Child process stopped");
        }

        Ok(())
    }

    /// Send a signal to the child process.
    pub fn signal(&self, sig: Signal) -> Result<()> {
        if let Some(pid) = self.pid {
            signal::kill(Pid::from_raw(pid as i32), sig)?;
        }
        Ok(())
    }

    /// Check if the child process is running.
    pub fn is_running(&self) -> bool {
        if let Some(pid) = self.pid {
            signal::kill(Pid::from_raw(pid as i32), None).is_ok()
        } else {
            false
        }
    }

    /// Get the child process PID.
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Get the child process state.
    pub fn state(&self) -> &ProcessState {
        &self.state
    }
}
