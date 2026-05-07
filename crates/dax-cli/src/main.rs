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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match cli.command {
        Command::Devices => commands::devices::run(),
        Command::Snap { device, out } => commands::snap::run(device, &out),
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
            "daxauth={default_level},dax_capture={default_level}"
        ))
    });

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}
