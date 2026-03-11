//! Unix signal handling for graceful shutdown.
//!
//! Handles:
//! - `SIGTERM` — systemd stop / kill
//! - `SIGINT`  — Ctrl-C in development
//!
//! On signal: stop accepting new connections, finish in-flight sessions,
//! clean up socket file, exit 0.

use anyhow::Result;
use tracing::info;

/// Wait for `SIGTERM` or `SIGINT` and return `Ok(())`.
///
/// This function blocks (async) until either signal is received.
/// After returning, the caller should cancel any [`tokio_util::sync::CancellationToken`]
/// used to stop the server accept loop.
///
/// # Errors
/// Returns `Err` if the signal handler cannot be installed (should not happen
/// in a normal Unix environment).
pub async fn wait_for_shutdown() -> Result<()> {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    tokio::select! {
        _ = sigterm.recv() => {
            info!("received SIGTERM, initiating graceful shutdown");
        }
        _ = sigint.recv() => {
            info!("received SIGINT, initiating graceful shutdown");
        }
    }

    Ok(())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn cancellation_token_propagates() {
        use tokio_util::sync::CancellationToken;
        let token = CancellationToken::new();
        let child = token.clone();
        token.cancel();
        // Child should be immediately cancelled
        assert!(child.is_cancelled());
    }
}
