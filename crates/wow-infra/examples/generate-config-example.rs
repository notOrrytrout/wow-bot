use std::{env, fs, path::PathBuf};
use wow_infra::config::app::AppConfig;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = workspace_root.join("config.example.json");
    let body = serde_json::to_string_pretty(&AppConfig::default())?;

    fs::write(&output, format!("{body}\n"))?;
    println!("Generated {}", output.display());
    Ok(())
}
