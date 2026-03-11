//! Unix socket server — accepts connections from PAM module.
//!
//! Each connection represents a single authentication attempt.
//! The server serializes access to the ML pipeline to avoid
//! concurrent camera usage.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use dax_auth_core::AuthPipeline;

use crate::session::SessionHandler;

/// Bound Unix domain socket server.
///
/// Accepts connections from PAM module clients. Each connection is dispatched
/// to a [`SessionHandler`] in a new Tokio task. The ML pipeline is serialized
/// via an [`Arc<Mutex<AuthPipeline>>`] — only one authentication runs at a time.
pub struct DaemonServer {
    /// Bound Unix domain socket listener.
    listener: UnixListener,
    /// Shared ML pipeline (serialized via Mutex).
    pipeline: Arc<Mutex<AuthPipeline>>,
    /// Token used to signal the accept loop to stop.
    cancel: CancellationToken,
    /// Path to the socket file — used to clean up on shutdown.
    socket_path: PathBuf,
}

impl DaemonServer {
    /// Bind a Unix domain socket at `socket_path` and return a ready server.
    ///
    /// Steps:
    /// 1. Creates the socket directory (mkdir -p).
    /// 2. Removes any stale socket file (leftover from a previous crash).
    /// 3. Binds the listener.
    /// 4. Sets socket permissions to `0660`.
    ///
    /// # Errors
    /// Returns an error if any of the above steps fail.
    pub async fn bind(
        socket_path: &Path,
        pipeline: Arc<Mutex<AuthPipeline>>,
        cancel: CancellationToken,
    ) -> anyhow::Result<Self> {
        // 1. Create the parent directory (e.g. /run/dax-auth/)
        if let Some(parent) = socket_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create socket dir {}", parent.display()))?;
        }

        // 2. Remove stale socket (ignore errors — file may not exist)
        let _ = tokio::fs::remove_file(socket_path).await;

        // 3. Bind
        let listener = UnixListener::bind(socket_path)
            .with_context(|| format!("bind Unix socket {}", socket_path.display()))?;

        // 4. Set permissions: srw-rw---- (0660)
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o660))
            .with_context(|| format!("chmod 0660 {}", socket_path.display()))?;

        info!(path = %socket_path.display(), "daemon socket bound");

        Ok(Self {
            listener,
            pipeline,
            cancel,
            socket_path: socket_path.to_owned(),
        })
    }

    /// Run the accept loop until the cancellation token is cancelled.
    ///
    /// Each accepted connection is handed off to a [`SessionHandler`] running
    /// in a new Tokio task. Accept errors are logged but do not stop the loop.
    ///
    /// On shutdown:
    /// 1. The cancellation token fires (`DaemonServer::cancel`).
    /// 2. The accept loop exits.
    /// 3. The socket file is removed.
    ///
    /// # Errors
    /// Returns an error only on fatal listener failures (rare in practice).
    pub async fn run(self) -> anyhow::Result<()> {
        let Self {
            listener,
            pipeline,
            cancel,
            socket_path,
        } = self;

        loop {
            tokio::select! {
                // Shutdown signal — exit gracefully.
                _ = cancel.cancelled() => {
                    info!("shutdown signal received, stopping accept loop");
                    break;
                }

                // New connection.
                result = listener.accept() => {
                    match result {
                        Ok((stream, _addr)) => {
                            let pipeline_clone = Arc::clone(&pipeline);
                            tokio::spawn(async move {
                                let handler = SessionHandler::new(stream, pipeline_clone);
                                if let Err(e) = handler.handle().await {
                                    warn!(error = %e, "session handler error");
                                }
                            });
                        }
                        Err(e) => {
                            // Accept errors are transient (EINTR, ECONNRESET) — keep serving.
                            error!(error = %e, "accept error");
                        }
                    }
                }
            }
        }

        // Remove socket file on clean shutdown.
        if let Err(e) = tokio::fs::remove_file(&socket_path).await {
            warn!(error = %e, path = %socket_path.display(), "failed to remove socket on shutdown");
        }

        Ok(())
    }
}
