use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fs, io, path::{Path, PathBuf}, time::UNIX_EPOCH};

use crate::config::runtime_data::ResolvedRuntimeDataPaths;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeAssetManifest {
    pub version: u32,
    pub generated_unix_s: u64,
    pub sources: RuntimeAssetSources,
    pub map_ids: Vec<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeAssetSources {
    pub dbc: AssetDirectory,
    pub maps: AssetDirectory,
    pub vmaps: AssetDirectory,
    pub mmaps: AssetDirectory,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssetDirectory {
    pub path: PathBuf,
    pub files: u64,
    pub newest_modified_unix_s: Option<u64>,
}

pub fn generate_runtime_asset_manifest(paths: &ResolvedRuntimeDataPaths, output: &Path) -> io::Result<RuntimeAssetManifest> {
    let dbc = scan_dir(&paths.dbc)?;
    let maps = scan_dir(&paths.maps)?;
    let vmaps = scan_dir(&paths.vmaps)?;
    let mmaps = scan_dir(&paths.mmaps)?;
    let mut ids = BTreeSet::new();
    collect_map_ids(&paths.maps, &mut ids)?;
    collect_map_ids(&paths.vmaps, &mut ids)?;
    collect_map_ids(&paths.mmaps, &mut ids)?;
    let generated_unix_s = std::time::SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let manifest = RuntimeAssetManifest {
        version: 1,
        generated_unix_s,
        sources: RuntimeAssetSources { dbc, maps, vmaps, mmaps },
        map_ids: ids.into_iter().collect(),
    };
    if let Some(parent) = output.parent() { fs::create_dir_all(parent)?; }
    let tmp = output.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?)?;
    if output.exists() { fs::remove_file(output)?; }
    fs::rename(tmp, output)?;
    Ok(manifest)
}

fn scan_dir(path: &Path) -> io::Result<AssetDirectory> {
    let mut files = 0_u64;
    let mut newest = None;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_file() {
            files += 1;
            if let Ok(modified) = metadata.modified().and_then(|value| value.duration_since(UNIX_EPOCH).map_err(io::Error::other)) {
                newest = Some(newest.map_or(modified.as_secs(), |current: u64| current.max(modified.as_secs())));
            }
        }
    }
    Ok(AssetDirectory { path: path.to_path_buf(), files, newest_modified_unix_s: newest })
}

fn collect_map_ids(path: &Path, ids: &mut BTreeSet<u32>) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() { continue; }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let digits: String = name.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.len() >= 3 {
            if let Ok(id) = digits[..3].parse::<u32>() { ids.insert(id); }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn map_id_comes_from_first_three_filename_digits() {
        let dir = std::env::temp_dir().join(format!("wow-bot-manifest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("0012345.map"), b"x").unwrap();
        fs::write(dir.join("530.mmap"), b"x").unwrap();
        let mut ids = BTreeSet::new();
        collect_map_ids(&dir, &mut ids).unwrap();
        assert!(ids.contains(&1));
        assert!(ids.contains(&530));
        let _ = fs::remove_dir_all(&dir);
    }
}
