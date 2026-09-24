use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use crate::NavigationError;

const GRID_SIZE: f32 = 533.333_3;
const MAP_RESOLUTION: f32 = 128.0;
const MAP_HEIGHT_NO_HEIGHT: u32 = 0x0001;
const MAP_HEIGHT_AS_INT16: u32 = 0x0002;
const MAP_HEIGHT_AS_INT8: u32 = 0x0004;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerrainKind {
    Ground,
    Water,
    Air,
    Unknown,
}

/// Read-only AzerothCore terrain-height source. Parsed tiles are cached per worker.
#[derive(Clone)]
pub struct TerrainSampler {
    maps_dir: PathBuf,
    tiles: Arc<Mutex<HashMap<PathBuf, Arc<TerrainTile>>>>,
}

impl TerrainSampler {
    pub fn new(maps_dir: impl Into<PathBuf>) -> Result<Self, NavigationError> {
        let maps_dir = maps_dir.into();
        if !maps_dir.is_dir() {
            return Err(NavigationError::MissingNavigationData);
        }
        Ok(Self {
            maps_dir,
            tiles: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn maps_dir(&self) -> &Path {
        &self.maps_dir
    }

    pub fn ground_height(&self, map: u32, x: f32, y: f32) -> Result<f32, NavigationError> {
        let path = map_tile_path(&self.maps_dir, map, x, y)?;
        let tile = self.tile(path)?;
        tile.height(x, y)
    }

    fn tile(&self, path: PathBuf) -> Result<Arc<TerrainTile>, NavigationError> {
        if let Some(tile) = self
            .tiles
            .lock()
            .map_err(|_| NavigationError::MissingNavigationData)?
            .get(&path)
            .cloned()
        {
            return Ok(tile);
        }
        let bytes = fs::read(&path).map_err(|_| NavigationError::MissingNavigationData)?;
        let tile = Arc::new(TerrainTile::parse(&bytes)?);
        let mut cache = self
            .tiles
            .lock()
            .map_err(|_| NavigationError::MissingNavigationData)?;
        if cache.len() >= 64 && !cache.contains_key(&path) {
            if let Some(old) = cache.keys().next().cloned() {
                cache.remove(&old);
            }
        }
        cache.insert(path, tile.clone());
        Ok(tile)
    }
}

pub(crate) fn map_tile_path(
    root: &Path,
    map: u32,
    x: f32,
    y: f32,
) -> Result<PathBuf, NavigationError> {
    if !x.is_finite() || !y.is_finite() {
        return Err(NavigationError::InvalidCoordinate);
    }
    let gx = (32.0 - x / GRID_SIZE) as i32;
    let gy = (32.0 - y / GRID_SIZE) as i32;
    if !(0..64).contains(&gx) || !(0..64).contains(&gy) {
        return Err(NavigationError::InvalidCoordinate);
    }
    Ok(root.join(format!("{map:03}{gx:02}{gy:02}.map")))
}

enum HeightGrid {
    Flat,
    U8 {
        v9: Vec<u8>,
        v8: Vec<u8>,
        scale: f32,
    },
    U16 {
        v9: Vec<u16>,
        v8: Vec<u16>,
        scale: f32,
    },
    Float {
        v9: Vec<f32>,
        v8: Vec<f32>,
    },
}

struct TerrainTile {
    base: f32,
    heights: HeightGrid,
}

impl TerrainTile {
    fn parse(bytes: &[u8]) -> Result<Self, NavigationError> {
        if bytes.get(0..4) != Some(b"MAPS") || read_u32(bytes, 4)? != 9 {
            return Err(NavigationError::MissingNavigationData);
        }
        let offset = read_u32(bytes, 20)? as usize;
        if offset == 0 || bytes.get(offset..offset + 4) != Some(b"MHGT") {
            return Err(NavigationError::MissingNavigationData);
        }
        let flags = read_u32(bytes, offset + 4)?;
        let base = read_f32(bytes, offset + 8)?;
        let max = read_f32(bytes, offset + 12)?;
        if !base.is_finite() || !max.is_finite() {
            return Err(NavigationError::InvalidCoordinate);
        }
        let data = offset + 16;
        let heights = if flags & MAP_HEIGHT_NO_HEIGHT != 0 {
            HeightGrid::Flat
        } else if flags & MAP_HEIGHT_AS_INT16 != 0 {
            HeightGrid::U16 {
                v9: read_u16_vec(bytes, data, 129 * 129)?,
                v8: read_u16_vec(bytes, data + 129 * 129 * 2, 128 * 128)?,
                scale: (max - base) / 65_535.0,
            }
        } else if flags & MAP_HEIGHT_AS_INT8 != 0 {
            HeightGrid::U8 {
                v9: read_slice(bytes, data, 129 * 129)?.to_vec(),
                v8: read_slice(bytes, data + 129 * 129, 128 * 128)?.to_vec(),
                scale: (max - base) / 255.0,
            }
        } else {
            HeightGrid::Float {
                v9: read_f32_vec(bytes, data, 129 * 129)?,
                v8: read_f32_vec(bytes, data + 129 * 129 * 4, 128 * 128)?,
            }
        };
        Ok(Self { base, heights })
    }

    fn height(&self, world_x: f32, world_y: f32) -> Result<f32, NavigationError> {
        let mut x = MAP_RESOLUTION * (32.0 - world_x / GRID_SIZE);
        let mut y = MAP_RESOLUTION * (32.0 - world_y / GRID_SIZE);
        if !x.is_finite() || !y.is_finite() {
            return Err(NavigationError::InvalidCoordinate);
        }
        let xi = x as i32;
        let yi = y as i32;
        x -= xi as f32;
        y -= yi as f32;
        let row = (xi & 127) as usize;
        let col = (yi & 127) as usize;
        let v9 = row * 129 + col;
        let v8 = row * 128 + col;
        let value = match &self.heights {
            HeightGrid::Flat => self.base,
            HeightGrid::U8 {
                v9: a,
                v8: b,
                scale,
            } => sample_height(a, b, v9, v8, x, y, *scale, self.base),
            HeightGrid::U16 {
                v9: a,
                v8: b,
                scale,
            } => sample_height(a, b, v9, v8, x, y, *scale, self.base),
            HeightGrid::Float { v9: a, v8: b } => sample_height(a, b, v9, v8, x, y, 1.0, 0.0),
        };
        value
            .is_finite()
            .then_some(value)
            .ok_or(NavigationError::InvalidCoordinate)
    }
}

fn sample_height<T: Copy + Into<f32>>(
    v9: &[T],
    v8: &[T],
    index9: usize,
    index8: usize,
    x: f32,
    y: f32,
    scale: f32,
    base: f32,
) -> f32 {
    interpolate(
        v9[index9].into(),
        v9[index9 + 129].into(),
        v9[index9 + 1].into(),
        v9[index9 + 130].into(),
        v8[index8].into(),
        x,
        y,
    ) * scale
        + base
}

fn interpolate(h1: f32, h2: f32, h3: f32, h4: f32, h5: f32, x: f32, y: f32) -> f32 {
    let (a, b, c) = if x + y < 1.0 {
        if x > y {
            (h2 - h1, 2.0 * h5 - h2 - h1, h1)
        } else {
            (2.0 * h5 - h3 - h1, h3 - h1, h1)
        }
    } else if x > y {
        (h2 + h4 - 2.0 * h5, h4 - h2, 2.0 * h5 - h4)
    } else {
        (h4 - h3, h3 + h4 - 2.0 * h5, 2.0 * h5 - h4)
    };
    a * x + b * y + c
}

fn read_slice(bytes: &[u8], offset: usize, len: usize) -> Result<&[u8], NavigationError> {
    bytes
        .get(
            offset
                ..offset
                    .checked_add(len)
                    .ok_or(NavigationError::MissingNavigationData)?,
        )
        .ok_or(NavigationError::MissingNavigationData)
}
fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, NavigationError> {
    let b = read_slice(bytes, offset, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
fn read_f32(bytes: &[u8], offset: usize) -> Result<f32, NavigationError> {
    Ok(f32::from_bits(read_u32(bytes, offset)?))
}
fn read_u16_vec(bytes: &[u8], offset: usize, count: usize) -> Result<Vec<u16>, NavigationError> {
    let raw = read_slice(
        bytes,
        offset,
        count
            .checked_mul(2)
            .ok_or(NavigationError::MissingNavigationData)?,
    )?;
    Ok(raw
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect())
}
fn read_f32_vec(bytes: &[u8], offset: usize, count: usize) -> Result<Vec<f32>, NavigationError> {
    let raw = read_slice(
        bytes,
        offset,
        count
            .checked_mul(4)
            .ok_or(NavigationError::MissingNavigationData)?,
    )?;
    let values: Vec<f32> = raw
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    if values.iter().any(|v| !v.is_finite()) {
        return Err(NavigationError::InvalidCoordinate);
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn flat_azerothcore_tile_returns_ground_height() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("wow-nav-terrain-{unique}"));
        std::fs::create_dir_all(&root).unwrap();
        let mut bytes = vec![0_u8; 36 + 16];
        bytes[0..4].copy_from_slice(b"MAPS");
        bytes[4..8].copy_from_slice(&9_u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&36_u32.to_le_bytes());
        bytes[36..40].copy_from_slice(b"MHGT");
        bytes[40..44].copy_from_slice(&MAP_HEIGHT_NO_HEIGHT.to_le_bytes());
        bytes[44..48].copy_from_slice(&42.0_f32.to_le_bytes());
        bytes[48..52].copy_from_slice(&42.0_f32.to_le_bytes());
        let path = map_tile_path(&root, 1, 0.0, 0.0).unwrap();
        std::fs::write(path, bytes).unwrap();
        let terrain = TerrainSampler::new(&root).unwrap();
        assert_eq!(terrain.ground_height(1, 0.0, 0.0).unwrap(), 42.0);
        let _ = std::fs::remove_dir_all(root);
    }
}
