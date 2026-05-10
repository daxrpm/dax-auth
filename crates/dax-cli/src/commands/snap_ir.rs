use std::path::Path;

use anyhow::{bail, Context, Result};
use dax_capture::{Camera, IrCamera, PixelFormat};
use image::{ImageBuffer, Luma};
use tracing::info;

pub fn run(device: u32, out: &Path) -> Result<()> {
    let mut camera: IrCamera =
        Camera::open_ir(device).with_context(|| format!("opening IR camera index {device}"))?;

    let frame = camera
        .capture()
        .with_context(|| format!("capturing IR frame from device {device}"))?;

    if frame.format() != PixelFormat::Gray8 {
        bail!(
            "unexpected frame format: {:?}, GRAY8 expected for IR snap",
            frame.format()
        );
    }

    let image: ImageBuffer<Luma<u8>, Vec<u8>> =
        ImageBuffer::from_raw(frame.width(), frame.height(), frame.data().to_vec())
            .context("frame buffer did not match declared dimensions")?;

    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating output directory {}", parent.display()))?;
        }
    }

    image
        .save(out)
        .with_context(|| format!("writing IR image to {}", out.display()))?;

    info!(
        path = %out.display(),
        width = frame.width(),
        height = frame.height(),
        "ir snapshot saved"
    );
    println!(
        "Saved {}x{} IR snapshot to {}",
        frame.width(),
        frame.height(),
        out.display()
    );
    Ok(())
}
