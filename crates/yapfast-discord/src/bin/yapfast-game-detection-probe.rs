//! Measure only the game detector, using a downloaded or custom catalogue.
use anyhow::{Context, Result};

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let file = args
        .next()
        .context("usage: yapfast-game-detection-probe CATALOGUE.json [SAMPLES]")?;
    let samples = args
        .next()
        .map(|s| s.to_string_lossy().parse())
        .transpose()?
        .unwrap_or(30);
    let report = yapfast_discord::game_detection::measure(std::path::Path::new(&file), samples)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
