use anyhow::{Context, Result};
use std::{env, path::PathBuf};
use wow_infra::{
    config::{app::AppConfig, data_dir::AppPaths},
    world_knowledge::generate::generate_runtime_asset_manifest,
};

fn main() -> Result<()> {
    let paths = AppPaths::discover().map_err(anyhow::Error::msg)?;
    paths.ensure().map_err(anyhow::Error::msg)?;
    let config = AppConfig::load(&paths.config).map_err(anyhow::Error::msg)
        .with_context(|| format!("load {} before generating world data", paths.config.display()))?;
    let out: PathBuf = env::args().nth(1)
        .map(Into::into)
        .unwrap_or_else(|| paths.generated.join("runtime-assets.json"));
    let resolved = config.runtime.runtime_data.resolved();
    let manifest = generate_runtime_asset_manifest(&resolved, &out)
        .with_context(|| format!("generate runtime asset manifest at {}", out.display()))?;
    println!(
        "wrote {} (dbc={}, maps={}, vmaps={}, mmaps={}, map_ids={})",
        out.display(),
        manifest.sources.dbc.files,
        manifest.sources.maps.files,
        manifest.sources.vmaps.files,
        manifest.sources.mmaps.files,
        manifest.map_ids.len(),
    );
    Ok(())
}
