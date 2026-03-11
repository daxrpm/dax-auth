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

    /// Download default ONNX models to /usr/share/dax-auth/models/
    DownloadModels {
        /// Target directory (default: /usr/share/dax-auth/models/)
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

async fn cmd_enroll(_user: Option<String>, _label: Option<String>) -> anyhow::Result<()> {
    println!("🎥 Starting guided face enrollment...");
    // TODO Phase 2: guided enrollment with camera preview
    todo!("implement enrollment command")
}

async fn cmd_list(_user: Option<String>) -> anyhow::Result<()> {
    // TODO Phase 2
    todo!("implement list command")
}

async fn cmd_remove(_user: Option<String>, _index: usize, _yes: bool) -> anyhow::Result<()> {
    // TODO Phase 2
    todo!("implement remove command")
}

async fn cmd_clear(_user: Option<String>, _yes: bool) -> anyhow::Result<()> {
    // TODO Phase 2
    todo!("implement clear command")
}

async fn cmd_test(_verbose: bool) -> anyhow::Result<()> {
    // TODO Phase 2
    todo!("implement test command")
}

async fn cmd_status() -> anyhow::Result<()> {
    // TODO Phase 1: query daemon via socket
    todo!("implement status command")
}

async fn cmd_download_models(_dir: Option<std::path::PathBuf>) -> anyhow::Result<()> {
    // TODO Phase 2
    todo!("implement download-models command")
}
