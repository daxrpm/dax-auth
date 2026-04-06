//! # dax-auth CLI
//!
//! Command-line tool for managing dax-auth facial authentication.
//!
//! ## Commands
//! - `dax-auth enroll` — enroll your face (guided)
//! - `dax-auth list` — list enrolled faces
//! - `dax-auth remove <index>` — remove an enrolled face
//! - `dax-auth clear` — remove all enrolled faces
//! - `dax-auth test` — test camera and recognition
//! - `dax-auth status` — show daemon status
//! - `dax-auth download-models` — download default ONNX models
//! - `dax-auth config` — open config in editor

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

use clap::{Parser, Subcommand};
use dax_auth_core::{pipeline::AuthPipeline, store::FaceStore, CoreConfig};
use std::path::Path;

/// Path to the production config file.
const CONFIG_PATH: &str = "/etc/dax-auth/config.toml";

/// Path to the daemon Unix socket (used by `status` command).
const SOCKET_PATH: &str = "/run/dax-auth/daemon.sock";

#[derive(Parser)]
#[command(
    name = "dax-auth",
    version = env!("CARGO_PKG_VERSION"),
    about = "Facial authentication for Linux",
    long_about = "dax-auth: Windows Hello-style facial authentication for Linux\nBacked by ArcFace + ONNX Runtime"
)]
struct Cli {
    /// Username to operate on (defaults to current user)
    #[arg(short = 'u', long, global = true)]
    user: Option<String>,

    /// Suppress prompts (answer yes to everything)
    #[arg(short = 'y', long, global = true)]
    yes: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Enroll a new face for authentication
    Enroll {
        /// Label for this face (e.g., "with glasses", "default")
        #[arg(short, long)]
        label: Option<String>,
    },

    /// List enrolled faces
    List,

    /// Remove a specific enrolled face by index
    Remove {
        /// Index of the face to remove (from `list` command)
        index: usize,
    },

    /// Remove all enrolled faces for the user
    Clear,

    /// Test camera and recognition pipeline
    Test {
        /// Show verbose inference output
        #[arg(short, long)]
        verbose: bool,
    },

    /// Show daemon status and loaded models
    Status,

    /// Download default ONNX models to /var/lib/dax-auth/models/
    DownloadModels {
        /// Target directory (default: /var/lib/dax-auth/models/)
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
    },

    /// Print the current version
    Version,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Enroll { label } => cmd_enroll(cli.user, label).await,
        Command::List => cmd_list(cli.user).await,
        Command::Remove { index } => cmd_remove(cli.user, index, cli.yes).await,
        Command::Clear => cmd_clear(cli.user, cli.yes).await,
        Command::Test { verbose } => cmd_test(verbose).await,
        Command::Status => cmd_status().await,
        Command::DownloadModels { dir } => cmd_download_models(dir).await,
        Command::Version => {
            println!("dax-auth {}", env!("CARGO_PKG_VERSION"));
            return;
        }
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

// ─── Task 2.3.1 helpers ───────────────────────────────────────────────────────

/// Resolve the target username from the `--user` flag or the calling user's env.
fn resolve_username(user: Option<String>) -> anyhow::Result<String> {
    if let Some(u) = user {
        return Ok(u);
    }
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .map_err(|_| anyhow::anyhow!("Cannot determine current username. Use --user flag."))
}

/// Format a Unix timestamp as a human-readable UTC string.
///
/// Uses chrono (already a workspace dep) for ISO 8601 formatting.
fn format_timestamp(ts: u64) -> String {
    use chrono::{DateTime, Utc};
    let dt = DateTime::<Utc>::from_timestamp(ts as i64, 0);
    match dt {
        Some(d) => d.format("%Y-%m-%d %H:%M UTC").to_string(),
        None => format!("(unix: {ts})"),
    }
}

// ─── Task 2.3.2 — cmd_enroll ──────────────────────────────────────────────────

/// Enroll a new face for the given user.
///
/// Loads models, opens the camera, captures and verifies a live face, then
/// stores the resulting embedding encrypted in the face store.
async fn cmd_enroll(user: Option<String>, label: Option<String>) -> anyhow::Result<()> {
    let username = resolve_username(user)?;
    let config = CoreConfig::load(Path::new(CONFIG_PATH))?;

    // Derive the storage directory from the models directory.
    let storage_dir = config
        .models_dir
        .parent()
        .map(|p| p.join("users"))
        .unwrap_or_else(|| std::path::PathBuf::from("/var/lib/dax-auth/users"));

    let store = FaceStore::open(&storage_dir)?;

    // Check current enrollment count against max_faces limit (default 5).
    const MAX_FACES: usize = 5;
    let current = store.count(&username)?;
    if current >= MAX_FACES {
        anyhow::bail!(
            "Maximum faces enrolled ({current}/{MAX_FACES}). Remove one first with `dax-auth remove <index>`."
        );
    }

    // Load the full pipeline (detection + liveness + recognition).
    println!("Loading models...");
    let mut pipeline = AuthPipeline::initialize(config)?;

    println!("Look at the camera. Hold still...");
    let embedding = match pipeline.capture_and_embed().await {
        Ok(e) => e,
        Err(e) => {
            // Workaround: ONNX Runtime dynamic library may crash during session
            // teardown in short-lived CLI processes on some hosts.
            // Intentionally leak the pipeline; process exit reclaims memory.
            std::mem::forget(pipeline);
            return Err(e.into());
        }
    };

    // See workaround note above.
    std::mem::forget(pipeline);

    let count = store.enroll_with_label(&username, embedding, label)?;
    println!("Face enrolled successfully. You now have {count} enrolled face(s).");
    Ok(())
}

// ─── Task 2.3.3 — cmd_list, cmd_remove, cmd_clear ────────────────────────────

/// List all enrolled faces for the given user.
async fn cmd_list(user: Option<String>) -> anyhow::Result<()> {
    let username = resolve_username(user)?;
    let config = CoreConfig::load(Path::new(CONFIG_PATH))?;
    let storage_dir = config
        .models_dir
        .parent()
        .map(|p| p.join("users"))
        .unwrap_or_else(|| std::path::PathBuf::from("/var/lib/dax-auth/users"));
    let store = FaceStore::open(&storage_dir)?;

    let metas = store.list_metadata(&username)?;
    if metas.is_empty() {
        println!("No enrolled faces for '{username}'.");
        return Ok(());
    }

    println!("Enrolled faces for '{username}' ({} total):", metas.len());
    for m in &metas {
        let dt = format_timestamp(m.enrolled_at);
        println!("  #{:<3} {:<35} ({})", m.index, m.label, dt);
    }
    Ok(())
}

/// Remove a specific enrolled face by index.
async fn cmd_remove(user: Option<String>, index: usize, yes: bool) -> anyhow::Result<()> {
    let username = resolve_username(user)?;
    let config = CoreConfig::load(Path::new(CONFIG_PATH))?;
    let storage_dir = config
        .models_dir
        .parent()
        .map(|p| p.join("users"))
        .unwrap_or_else(|| std::path::PathBuf::from("/var/lib/dax-auth/users"));
    let store = FaceStore::open(&storage_dir)?;

    // Validate the index before prompting.
    let metas = store.list_metadata(&username)?;
    let face = metas.get(index).ok_or_else(|| {
        let max = metas.len().saturating_sub(1);
        anyhow::anyhow!("index {index} out of range (0\u{2013}{max})")
    })?;

    if !yes {
        use std::io::Write as _;
        print!(
            "Remove face #{} '{}' enrolled {}? [y/N] ",
            face.index,
            face.label,
            format_timestamp(face.enrolled_at)
        );
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    store.remove(&username, index)?;
    let remaining = store.count(&username)?;
    println!("Face #{index} removed. {remaining} face(s) remaining.");
    Ok(())
}

/// Remove all enrolled faces for the given user, with optional confirmation prompt.
async fn cmd_clear(user: Option<String>, yes: bool) -> anyhow::Result<()> {
    let username = resolve_username(user)?;
    let config = CoreConfig::load(Path::new(CONFIG_PATH))?;
    let storage_dir = config
        .models_dir
        .parent()
        .map(|p| p.join("users"))
        .unwrap_or_else(|| std::path::PathBuf::from("/var/lib/dax-auth/users"));
    let store = FaceStore::open(&storage_dir)?;

    let count = store.count(&username)?;
    if count == 0 {
        println!("No enrolled faces to clear for '{username}'.");
        return Ok(());
    }

    if !yes {
        use std::io::Write as _;
        print!(
            "Remove ALL {count} enrolled face(s) for '{username}'? This cannot be undone. [y/N] "
        );
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    store.clear(&username)?;
    println!("Cleared {count} enrolled face(s) for '{username}'.");
    Ok(())
}

// ─── Task 2.3.4 — cmd_test ───────────────────────────────────────────────────

/// Run a full pipeline diagnostic and print a status table.
///
/// Exit code: 0 if all stages pass, 1 if any stage fails.
async fn cmd_test(verbose: bool) -> anyhow::Result<()> {
    use dax_auth_camera::CameraDevice;

    let config = CoreConfig::load(Path::new(CONFIG_PATH))?;

    println!("dax-auth pipeline test");
    println!("{}", "─".repeat(45));

    let mut all_ok = true;

    // ── Camera check ──────────────────────────────────────────────────────────
    print!("Camera:          ");
    match CameraDevice::best_available() {
        Ok(dev) => {
            println!("{} ({:?}) — OK", dev.path, dev.kind);
            if verbose {
                println!("                 Resolution: {}x{}", dev.width, dev.height);
            }
        }
        Err(e) => {
            println!("FAIL ({e})");
            // Without a camera the rest of the test cannot run.
            println!("{}", "─".repeat(45));
            println!("Result: FAIL");
            std::process::exit(1);
        }
    }

    // ── Model file presence check ─────────────────────────────────────────────
    // Required for base auth flow: detector + recognizer.
    let required_models = [
        config.models_dir.join(&config.detector_model),
        config.models_dir.join(&config.recognizer_model),
    ];

    let mut required_ok = true;
    for path in &required_models {
        if !path.exists() {
            if required_ok {
                // Print header only once.
                print!("Models:          ");
            }
            println!("FAIL — missing required: {}", path.display());
            required_ok = false;
        }
    }

    if required_ok {
        print!("Models:          ");
        println!("required models present — OK");
        if verbose {
            for path in &required_models {
                println!("                 required: {}", path.display());
            }
        }
    } else {
        println!("  Hint: run `sudo dax-auth download-models`");
        println!("{}", "─".repeat(45));
        println!("Result: FAIL");
        std::process::exit(1);
    }

    // Optional hardening model: anti-spoof. Missing file is a warning, not FAIL.
    let anti_spoof_path = config.models_dir.join(&config.anti_spoof_model);
    print!("Liveness model:  ");
    if anti_spoof_path.exists() {
        println!("{} — OK", anti_spoof_path.display());
    } else {
        println!(
            "missing optional model: {} — WARNING (reduced anti-spoof security)",
            anti_spoof_path.display()
        );
        println!("                 add minifasnet_v2.onnx for stronger liveness checks");
    }

    // ── Pipeline load check ───────────────────────────────────────────────────
    let pipeline_start = std::time::Instant::now();
    print!("Loading models:  ");
    let mut pipeline = match AuthPipeline::initialize(config.clone()) {
        Ok(p) => {
            let elapsed = pipeline_start.elapsed().as_millis();
            println!("OK ({elapsed}ms)");
            p
        }
        Err(e) => {
            println!("FAIL ({e})");
            println!("{}", "─".repeat(45));
            println!("Result: FAIL");
            std::process::exit(1);
        }
    };

    // ── Capture and embed ─────────────────────────────────────────────────────
    println!("Capturing frame (look at camera)...");
    let capture_start = std::time::Instant::now();
    let embedding_result = pipeline.capture_and_embed().await;
    let capture_elapsed = capture_start.elapsed().as_millis();

    match &embedding_result {
        Ok(emb) => {
            print!("Face detection:  ");
            println!("1 face detected — OK");
            print!("Liveness:        ");
            println!("LIVE");
            print!("Embedding:       ");
            println!("{}-dim — OK", emb.data.len());
            if verbose {
                let norm: f32 = emb.data.iter().map(|x| x * x).sum::<f32>().sqrt();
                println!("                 norm = {norm:.4}");
                println!("                 inference time: {capture_elapsed}ms");
            }
        }
        Err(e) => {
            print!("Face detection:  ");
            println!("FAIL ({e})");
            all_ok = false;
        }
    }

    // ── Enrolled faces check ─────────────────────────────────────────────────
    let storage_dir = config
        .models_dir
        .parent()
        .map(|p| p.join("users"))
        .unwrap_or_else(|| std::path::PathBuf::from("/var/lib/dax-auth/users"));

    let username = std::env::var("USER").unwrap_or_else(|_| "unknown".into());

    if let Ok(store) = FaceStore::open(&storage_dir) {
        let enrolled_count = store.count(&username).unwrap_or(0);

        if enrolled_count == 0 {
            print!("Match:           ");
            println!("no enrolled faces — SKIP");
        } else if let Ok(embedding) = &embedding_result {
            // Compare the captured embedding against all enrolled faces.
            if let Ok(user_emb) = store.load(&username) {
                let mut best_score: f32 = 0.0;
                for enrolled in &user_emb.embeddings {
                    let sim = embedding.cosine_similarity(enrolled);
                    if sim > best_score {
                        best_score = sim;
                    }
                }
                let threshold = config.thresholds.secure;
                let match_str = if best_score >= threshold {
                    "MATCH"
                } else {
                    "NO MATCH"
                };
                print!("Match:           ");
                println!(
                    "best score {best_score:.3} vs {enrolled_count} enrolled face(s) — {match_str} (threshold {threshold:.2})"
                );
                if best_score < threshold {
                    all_ok = false;
                }
            }
        }
    }

    println!("{}", "─".repeat(45));
    if all_ok {
        // Workaround: ONNX Runtime dynamic library may crash during session
        // teardown in short-lived CLI processes on some hosts.
        std::mem::forget(pipeline);
        println!("Result: PASS");
    } else {
        println!("Result: FAIL");
        std::process::exit(1);
    }

    Ok(())
}

// ─── Task 2.3.5 — cmd_status (included in scope per prompt) ──────────────────

/// Check whether the dax-authd daemon is running by connecting to its socket.
///
/// Exit code: 0 if running, 1 if not.
async fn cmd_status() -> anyhow::Result<()> {
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    print!("dax-authd ({SOCKET_PATH})... ");

    // Connect with a 2-second timeout via a spawned blocking task.
    let socket_path = SOCKET_PATH.to_owned();
    let result = tokio::task::spawn_blocking(move || {
        // UnixStream::connect does not have a native timeout in std.
        // We use a separate thread and channel to enforce the timeout.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let r = UnixStream::connect(&socket_path);
            let _ = tx.send(r);
        });
        rx.recv_timeout(Duration::from_secs(2))
    })
    .await?;

    match result {
        Ok(Ok(_stream)) => {
            println!("running");
            println!("  socket: {SOCKET_PATH}");
            Ok(())
        }
        Ok(Err(e)) => {
            println!("not running ({e})");
            eprintln!("  Start with: systemctl start dax-authd");
            std::process::exit(1);
        }
        Err(_timeout) => {
            println!("not responding (timeout)");
            eprintln!("  Start with: systemctl start dax-authd");
            std::process::exit(1);
        }
    }
}

// ─── cmd_download_models ──────────────────────────────────────────────────────

/// Download default ONNX models by delegating to `scripts/download_models.sh`.
async fn cmd_download_models(dir: Option<std::path::PathBuf>) -> anyhow::Result<()> {
    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("scripts/download_models.sh");
    if let Some(d) = dir {
        cmd.env("DAX_AUTH_MODELS_DIR", d);
    }
    let status = cmd.status().await?;
    if !status.success() {
        anyhow::bail!("download-models script failed");
    }
    Ok(())
}
