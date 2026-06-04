//! Signal handling for shims.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::broadcast;

/// Signal type for the shim.
#[derive(Debug, Clone, Copy)]
pub enum Signal {
    /// SIGTERM (termination).
    SigTerm,
    /// SIGINT (interrupt).
    SigInt,
    /// SIGHUP (hangup).
    SigHup,
}

/// Signal handler for the shim.
pub struct SignalHandler {
    /// Shutdown flag.
    shutdown: Arc<AtomicBool>,

    /// Signal sender.
    sender: broadcast::Sender<Signal>,
}

impl SignalHandler {
    /// Create a new signal handler.
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(16);
        let shutdown = Arc::new(AtomicBool::new(false));

        // Spawn signal handler
        let shutdown_clone = shutdown.clone();
        let sender_clone = sender.clone();

        tokio::spawn(async move {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("Failed to register SIGTERM handler");
            let mut sigint =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                    .expect("Failed to register SIGINT handler");
            let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
                .expect("Failed to register SIGHUP handler");

            loop {
                tokio::select! {
                    _ = sigterm.recv() => {
                        tracing::info!("Received SIGTERM");
                        shutdown_clone.store(true, Ordering::SeqCst);
                        let _ = sender_clone.send(Signal::SigTerm);
                    }
                    _ = sigint.recv() => {
                        tracing::info!("Received SIGINT");
                        shutdown_clone.store(true, Ordering::SeqCst);
                        let _ = sender_clone.send(Signal::SigInt);
                    }
                    _ = sighup.recv() => {
                        tracing::info!("Received SIGHUP");
                        let _ = sender_clone.send(Signal::SigHup);
                    }
                }
            }
        });

        Self { shutdown, sender }
    }

    /// Check if shutdown has been requested.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    /// Subscribe to signals.
    pub fn subscribe(&self) -> broadcast::Receiver<Signal> {
        self.sender.subscribe()
    }
}

impl Default for SignalHandler {
    fn default() -> Self {
        Self::new()
    }
}
