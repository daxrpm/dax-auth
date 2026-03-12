//! # dax-authd
//!
//! The dax-auth system daemon.
//!
//! ## Responsibilities
//! - Listens on a Unix domain socket for auth requests from `pam_dax_auth.so`
//! - Manages camera capture lifecycle
//! - Runs the ML inference pipeline (detection → liveness → recognition)
//! - Returns auth results to PAM callers
//! - Notifies systemd via `sd_notify` when ready
//!
//! ## Security model
//! - Runs as dedicated `dax-auth` system user (not root)
//! - Member of `video` group for camera access
//! - `/run/dax-auth/daemon.sock` is `srw-rw----` (dax-auth:dax-auth)
//! - PAM module (running as root) connects to the socket
//! - All biometric data is zeroed on drop

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

use std::sync::Arc;

use anyhow::Context as _;
use tokio::sync::Mutex;
use tracing::{error, info};

mod config;
mod server;
mod session;
mod signals;

use config::DaemonConfig;
use dax_auth_core::AuthPipeline;
use server::DaemonServer;

#[tokio::main]
async fn main() {
    // Initialize structured logging — send to journald if available, stderr otherwise
    init_logging();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "dax-authd starting"
    );

    if let Err(e) = run().await {
        // Use {e:#} to print the full anyhow error chain (cause-by-cause).
        error!(error = %format!("{e:#}"), "daemon exited with error");
        std::process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    // ── 1. Load config ─────────────────────────────────────────────────────────
    let config = DaemonConfig::load()
        .context("failed to load daemon configuration")?;

    info!(
        socket = %config.socket_path.display(),
        models_dir = %config.core.models_dir.display(),
        "config loaded"
    );

    // ── 2. Initialize AuthPipeline (eager model loading) ──────────────────────
    info!("loading ONNX models — this may take a few seconds on first run");
    let pipeline = AuthPipeline::initialize(config.core.clone())
        .context("failed to initialize auth pipeline")?;
    info!("models loaded, pipeline ready");

    let pipeline = Arc::new(Mutex::new(pipeline));

    // ── 3. Set up cancellation token ──────────────────────────────────────────
    let cancel = tokio_util::sync::CancellationToken::new();

    // ── 4. Bind Unix socket ────────────────────────────────────────────────────
    let server = DaemonServer::bind(&config.socket_path, Arc::clone(&pipeline), cancel.clone())
        .await
        .context("failed to bind Unix socket")?;

    // ── 5. Notify systemd: READY=1 ────────────────────────────────────────────
    sd_notify()?;
    info!("dax-authd ready — accepting connections on {}", config.socket_path.display());

    // ── 6. Spawn signal handler ───────────────────────────────────────────────
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        if let Err(e) = signals::wait_for_shutdown().await {
            tracing::error!(error = %e, "signal handler failed");
        }
        cancel_clone.cancel();
    });

    // ── 7. Run accept loop (blocks until shutdown) ────────────────────────────
    server.run().await?;

    info!("dax-authd stopped cleanly");
    Ok(())
}

/// Send `READY=1` to systemd via `$NOTIFY_SOCKET` if set.
///
/// Silently skips if `$NOTIFY_SOCKET` is not set (non-systemd environment).
///
/// # Errors
/// Returns an error if the socket cannot be created or the datagram cannot
/// be sent. This is non-fatal in development but should not happen under systemd.
fn sd_notify() -> anyhow::Result<()> {
    if let Ok(notify_socket) = std::env::var("NOTIFY_SOCKET") {
        use std::os::unix::net::UnixDatagram;
        let sock =
            UnixDatagram::unbound().context("failed to create unbound Unix datagram socket")?;
        sock.send_to(b"READY=1\n", &notify_socket)
            .with_context(|| format!("sd_notify: send to {notify_socket} failed"))?;
        tracing::debug!(socket = %notify_socket, "sd_notify READY=1 sent");
    }
    Ok(())
}

fn init_logging() {
    use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    // Try journald first (systemd environment), fall back to stderr
    let registry = tracing_subscriber::registry().with(filter);

    match tracing_journald::layer() {
        Ok(journald_layer) => {
            registry
                .with(journald_layer)
                .init();
        }
        Err(_) => {
            registry
                .with(fmt::layer().with_writer(std::io::stderr))
                .init();
        }
    }
}
