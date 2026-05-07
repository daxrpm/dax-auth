use anyhow::{Context, Result};
use dax_capture::Enumerator;

pub fn run() -> Result<()> {
    let enumerator = Enumerator::new();
    let devices = enumerator
        .list()
        .context("failed to enumerate camera devices")?;

    if devices.is_empty() {
        println!("No camera devices detected.");
        return Ok(());
    }

    println!("Detected {} camera device(s):\n", devices.len());
    for device in &devices {
        println!("  [{}] {}", device.index, device.name);
        if !device.description.is_empty() && device.description != device.name {
            println!("       description : {}", device.description);
        }
        if let Some(path) = &device.path {
            if !path.is_empty() {
                println!("       path        : {path}");
            }
        }
    }

    Ok(())
}
