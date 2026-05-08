use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod commands;

#[derive(Debug, Parser)]
#[command(
    name = "daxauth",
    version,
    about = "Face authentication for Linux — PAM-aware biometric stack."
)]
struct Cli {
    /// Increase logging verbosity. Repeat to raise the level (-v, -vv).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List the camera devices visible to the operating system.
    Devices,

    /// Capture a single frame from a camera and save it to disk.
    Snap {
        /// Camera index as reported by `daxauth devices`.
        #[arg(short, long, default_value_t = 0)]
        device: u32,

        /// Output path. Format is inferred from the extension.
        #[arg(short, long)]
        out: PathBuf,
    },

    /// Capture a single grayscale frame from an IR camera.
    SnapIr {
        /// Camera index as reported by `daxauth devices`.
        #[arg(short, long, default_value_t = 2)]
        device: u32,

        /// Output path. PNG is recommended for grayscale.
        #[arg(short, long)]
        out: PathBuf,
    },

    /// Run face detection on an image file.
    Detect {
        /// Path to the SCRFD ONNX model (e.g. `models/buffalo_s/det_500m.onnx`).
        #[arg(short, long)]
        model: PathBuf,

        /// Input image file (JPEG/PNG).
        #[arg(short, long, alias = "in")]
        input: PathBuf,

        /// Optional path to save an annotated copy with bounding boxes
        /// and landmarks drawn on top.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },

    /// Compute the embedding for the first face detected in an image.
    Embed {
        #[arg(short, long, alias = "det")]
        detector: PathBuf,

        #[arg(short, long, alias = "rec")]
        recognizer: PathBuf,

        #[arg(short, long, alias = "in")]
        input: PathBuf,

        /// Optional: save the 112×112 aligned face for visual debugging.
        #[arg(long)]
        aligned_out: Option<PathBuf>,
    },

    /// Compare the faces in two images and report cosine similarity.
    Compare {
        #[arg(short, long, alias = "det")]
        detector: PathBuf,

        #[arg(short, long, alias = "rec")]
        recognizer: PathBuf,

        #[arg(short, long)]
        a: PathBuf,

        #[arg(short, long)]
        b: PathBuf,
    },

    /// Run passive liveness check on the first face in an image.
    Liveness {
        #[arg(short, long, alias = "det")]
        detector: PathBuf,

        #[arg(short, long, alias = "live")]
        liveness_model: PathBuf,

        #[arg(short, long, alias = "in")]
        input: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match cli.command {
        Command::Devices => commands::devices::run(),
        Command::Snap { device, out } => commands::snap::run(device, &out),
        Command::SnapIr { device, out } => commands::snap_ir::run(device, &out),
        Command::Detect { model, input, out } => {
            commands::detect::run(&model, &input, out.as_deref())
        }
        Command::Embed {
            detector,
            recognizer,
            input,
            aligned_out,
        } => commands::embed::run(&detector, &recognizer, &input, aligned_out.as_deref()),
        Command::Compare {
            detector,
            recognizer,
            a,
            b,
        } => commands::compare::run(&detector, &recognizer, &a, &b),
        Command::Liveness {
            detector,
            liveness_model,
            input,
        } => commands::liveness::run(&detector, &liveness_model, &input),
    }
}

fn init_tracing(verbosity: u8) {
    let default_level = match verbosity {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!(
            "daxauth={default_level},dax_capture={default_level},dax_detect={default_level},dax_embed={default_level},dax_liveness={default_level}"
        ))
    });

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}
