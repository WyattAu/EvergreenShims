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

    /// Process state.
    state: ProcessState,
}

impl ChildProcess {
    /// Create a new child process manager.
    pub fn new(config: ProcessConfig) -> Self {
        Self {
            config,
            pid: None,
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
        self.pid = Some(child.id().unwrap_or(0));
        self.state = ProcessState::Running;

        tracing::info!("Child process started with PID {:?}", self.pid);
        Ok(())
    }

    /// Stop the child process gracefully.
    pub async fn stop(&mut self) -> Result<()> {
        if let Some(pid) = self.pid {
            tracing::info!("Stopping child process PID {}", pid);

            // Send SIGTERM
            self.state = ProcessState::Stopping;
            let _ = signal::kill(Pid::from_raw(pid as i32), Signal::SIGTERM);

            // Wait for graceful shutdown
            let timeout = std::time::Duration::from_secs(self.config.shutdown_timeout_secs);
            let start = std::time::Instant::now();

            while start.elapsed() < timeout {
                // Check if process is still running
                let result = signal::kill(Pid::from_raw(pid as i32), None);
                if result.is_err() {
                    // Process has exited
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }

            // Force kill if still running
            if start.elapsed() >= timeout {
                tracing::warn!("Child process PID {} did not exit gracefully, sending SIGKILL", pid);
                let _ = signal::kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
            }

            self.pid = None;
            self.state = ProcessState::Stopped;
            tracing::info!("Child process stopped");
        }

        Ok(())
    }

    /// Send a signal to the child process.
    pub fn signal(&self, signal: Signal) -> Result<()> {
        if let Some(pid) = self.pid {
            signal::kill(Pid::from_raw(pid as i32), signal)?;
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
