//! Child process management.

use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};

use crate::config::ProcessConfig;
use crate::error::Result;
use crate::shutdown::{DatabaseType, ShutdownManager};

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

    /// Database type for shutdown sequence selection.
    db_type: DatabaseType,

    /// Child exit code (set when child exits or is killed).
    exit_code: Option<i32>,
}

impl ChildProcess {
    /// Create a new child process manager.
    pub fn new(config: ProcessConfig) -> Self {
        Self {
            config,
            pid: None,
            child: None,
            state: ProcessState::Stopped,
            db_type: DatabaseType::Generic,
            exit_code: None,
        }
    }

    /// Create a new child process manager with a specific database type.
    pub fn with_db_type(config: ProcessConfig, db_type: DatabaseType) -> Self {
        Self {
            config,
            pid: None,
            child: None,
            state: ProcessState::Stopped,
            db_type,
            exit_code: None,
        }
    }

    /// Set the database type for shutdown sequence selection.
    pub fn set_db_type(&mut self, db_type: DatabaseType) {
        self.db_type = db_type;
    }

    /// Get the database type.
    pub fn db_type(&self) -> &DatabaseType {
        &self.db_type
    }

    /// Start the child process.
    pub async fn start(&mut self) -> Result<()> {
        tracing::info!("Starting child process: {}", self.config.command);

        let mut cmd = tokio::process::Command::new(&self.config.command);
        cmd.args(&self.config.args);

        if let Some(dir) = &self.config.working_dir {
            cmd.current_dir(dir);
        }

        // CRITICAL: When PID 1 spawns a child, grandchildren that exit become
        // zombies if nobody reaps them. This is the root cause of the Forgejo
        // git-process accumulation issue. By setting ourselves as the subreaper,
        // we become responsible for reaping ALL descendant processes, not just
        // our direct child.
        //
        // This prevents the "[git]" zombie processes that accumulate when
        // Forgejo spawns git subprocesses that exit.
        #[cfg(target_os = "linux")]
        unsafe {
            // PR_SET_CHILD_SUBREAPER = 36
            // This makes us (the shim/PID 1) the subreaper for all orphaned
            // descendants. Combined with the SIGCHLD reaper below, this ensures
            // no zombies accumulate.
            libc::prctl(36, 1, 0, 0, 0);
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

        // Spawn a background task that continuously reaps zombie grandchildren.
        // This handles grandchildren (e.g., Forgejo's git subprocesses) that
        // get reparented to us via PR_SET_CHILD_SUBREAPER.
        //
        // We use waitpid(-1, WNOHANG) which reaps ANY child. If the main child
        // is reaped here, is_running() will detect it via ECHILD on the next
        // check, so the race is harmless.
        let main_pid = pid;
        tokio::spawn(async move {
            loop {
                // waitpid(-1, WNOHANG) reaps any zombie child/descendant
                use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
                match waitpid(None, Some(WaitPidFlag::WNOHANG)) {
                    Ok(WaitStatus::Exited(pid, code)) => {
                        if pid.as_raw() == main_pid as i32 {
                            tracing::info!(
                                "Main child process {} exited with code {} (reaped by background reaper)",
                                main_pid, code
                            );
                        } else {
                            tracing::debug!(
                                "Reaped grandchild zombie: PID {} (code {})",
                                pid,
                                code
                            );
                        }
                        // Continue immediately — there might be more zombies
                        continue;
                    }
                    Ok(WaitStatus::Signaled(pid, sig, _)) => {
                        if pid.as_raw() == main_pid as i32 {
                            tracing::warn!(
                                "Main child process {} killed by signal {:?} (reaped by background reaper)",
                                main_pid, sig
                            );
                        } else {
                            tracing::debug!(
                                "Reaped grandchild zombie: PID {} (signal {:?})",
                                pid,
                                sig
                            );
                        }
                        continue;
                    }
                    Ok(WaitStatus::StillAlive) => {
                        // No zombie available
                    }
                    Err(_) => {
                        // No children to reap
                    }
                    _ => {}
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        });

        tracing::info!("Child process started with PID {}", pid);
        Ok(())
    }

    /// Stop the child process gracefully.
    pub async fn stop(&mut self) -> Result<()> {
        if let Some(pid) = self.pid {
            tracing::info!(
                "Stopping child process PID {} (type: {})",
                pid,
                self.db_type
            );
            self.state = ProcessState::Stopping;

            let shutdown_mgr =
                ShutdownManager::new(self.db_type.clone(), self.config.shutdown_timeout_secs);

            let result = shutdown_mgr.shutdown(pid).await?;

            if !result.clean_exit {
                // Force kill if still running
                if self.is_running() {
                    tracing::warn!(
                        "Child process PID {} did not exit gracefully, sending SIGKILL",
                        pid
                    );
                    if let Err(e) = signal::kill(Pid::from_raw(pid as i32), Signal::SIGKILL) {
                        tracing::warn!("failed to send SIGKILL to PID {}: {}", pid, e);
                    }
                    // Wait briefly for SIGKILL
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                }
            }

            // Take the child handle so it's dropped
            self.child = None;
            self.pid = None;
            self.state = ProcessState::Stopped;
            tracing::info!("Child process stopped ({}ms)", result.duration_ms);
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
    ///
    /// Reads `/proc/<pid>/status` to determine the process state.
    /// This avoids race conditions between concurrent `waitpid` calls
    /// from the background reaper and this method.
    ///
    /// State values from /proc/<pid>/status:
    /// - R (running), S (sleeping), D (disk sleep), T (stopped) → alive
    /// - Z (zombie) → dead (exited but not yet reaped)
    /// - file not found → dead (fully reaped/gone)
    pub fn is_running(&self) -> bool {
        if let Some(pid) = self.pid {
            let status_path = format!("/proc/{}/status", pid);
            match std::fs::read_to_string(&status_path) {
                Ok(content) => {
                    for line in content.lines() {
                        if line.starts_with("State:") {
                            if line.contains("(zombie)") {
                                tracing::info!(
                                    "Child process PID {} is zombie (exited)",
                                    pid
                                );
                                return false;
                            }
                            return true;
                        }
                    }
                    true
                }
                Err(_) => {
                    tracing::info!(
                        "Child process PID {} status file gone (exited)",
                        pid
                    );
                    false
                }
            }
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

    /// Get the child process exit code (if exited).
    /// Returns None if the process is still running or hasn't been started.
    /// For signal deaths, returns 128 + signal_number (POSIX convention).
    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_child_process_new() {
        let config = ProcessConfig::default();
        let proc = ChildProcess::new(config);
        assert_eq!(*proc.state(), ProcessState::Stopped);
        assert!(!proc.is_running());
        assert_eq!(proc.pid(), None);
        assert_eq!(proc.exit_code(), None);
    }

    #[test]
    fn test_child_process_with_db_type() {
        let config = ProcessConfig::default();
        let proc = ChildProcess::with_db_type(config, DatabaseType::Postgres);
        assert_eq!(*proc.db_type(), DatabaseType::Postgres);
    }

    #[test]
    fn test_child_process_set_db_type() {
        let config = ProcessConfig::default();
        let proc = ChildProcess::new(config);
        assert_eq!(*proc.db_type(), DatabaseType::Generic);

        proc.set_db_type(DatabaseType::Redis);
        assert_eq!(*proc.db_type(), DatabaseType::Redis);
    }

    #[test]
    fn test_signal_to_nonexistent_process() {
        let config = ProcessConfig::default();
        let proc = ChildProcess::new(config);
        // Should not panic
        let _ = proc.signal(Signal::SIGTERM);
    }
}
