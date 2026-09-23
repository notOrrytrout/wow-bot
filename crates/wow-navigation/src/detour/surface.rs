//! Bounded surface observations. These do not authorize a route or change Z.

use super::format::{NAV_GROUND, NAV_WATER};
use super::{DetourPoint, DetourTile};

pub const MAX_SURFACE_CANDIDATES: usize = 8;
// This wider window exposes remote floors for diagnosis. It is not a
// movement tolerance. Selection retains the existing ten-yard local window.
pub const SURFACE_SCAN_VERTICAL_YARDS: f32 = 160.0;
const LOCAL_SURFACE_VERTICAL_YARDS: f32 = 10.0;
const SURFACE_HORIZONTAL_YARDS: f32 = 0.05;
const CONTINUITY_PENALTY_YARDS: f32 = 0.5;

/// Immutable geometry plus a horizontal broad-phase index. Oversized or
/// malformed bounds stay in the fallback list so indexing cannot hide them.
#[derive(Debug)]
pub(super) struct IndexedTile {
    pub(super) tile: DetourTile,
    cells: std::collections::HashMap<(i32, i32), Vec<usize>>,
    fallback: Vec<usize>,
}

impl std::ops::Deref for IndexedTile {
    type Target = DetourTile;
    fn deref(&self) -> &Self::Target {
        &self.tile
    }
}

impl IndexedTile {
    const CELL_YARDS: f32 = 8.0;

    pub(super) fn new(tile: DetourTile) -> Self {
        let mut cells = std::collections::HashMap::<(i32, i32), Vec<usize>>::new();
        let mut fallback = Vec::new();
        let mut index_entries = 0_usize;
        for (index, polygon) in tile.polygons.iter().enumerate() {
            if polygon.polygon_type != 0 {
                continue;
            }
            let vertices: Vec<_> = polygon
                .vertices
                .iter()
                .filter_map(|&vertex| tile.vertices.get(usize::from(vertex)))
                .collect();
            if vertices.is_empty()
                || vertices.len() != polygon.vertices.len()
                || vertices
                    .iter()
                    .any(|vertex| !vertex[0].is_finite() || !vertex[2].is_finite())
            {
                fallback.push(index);
                continue;
            }
            let min_x = vertices.iter().map(|v| v[0]).fold(f32::INFINITY, f32::min);
            let max_x = vertices
                .iter()
                .map(|v| v[0])
                .fold(f32::NEG_INFINITY, f32::max);
            let min_z = vertices.iter().map(|v| v[2]).fold(f32::INFINITY, f32::min);
            let max_z = vertices
                .iter()
                .map(|v| v[2])
                .fold(f32::NEG_INFINITY, f32::max);
            let low = (Self::cell(min_x), Self::cell(min_z));
            let high = (Self::cell(max_x), Self::cell(max_z));
            let width = i64::from(high.0) - i64::from(low.0) + 1;
            let height = i64::from(high.1) - i64::from(low.1) + 1;
            if width > 64
                || height > 64
                || width * height > 256
                || index_entries.saturating_add((width * height) as usize) > 1_000_000
            {
                fallback.push(index);
                continue;
            }
            index_entries += (width * height) as usize;
            for x in low.0..=high.0 {
                for z in low.1..=high.1 {
                    cells.entry((x, z)).or_default().push(index);
                }
            }
        }
        Self {
            tile,
            cells,
            fallback,
        }
    }

    fn cell(value: f32) -> i32 {
        (value / Self::CELL_YARDS).floor() as i32
    }

    fn nearby(&self, point: [f32; 3]) -> Vec<usize> {
        if !point.iter().all(|value| value.is_finite()) {
            return Vec::new();
        }
        // Detour horizontal axes are WoW Y and X, in that order.
        let low = (
            Self::cell(point[1] - SURFACE_HORIZONTAL_YARDS),
            Self::cell(point[0] - SURFACE_HORIZONTAL_YARDS),
        );
        let high = (
            Self::cell(point[1] + SURFACE_HORIZONTAL_YARDS),
            Self::cell(point[0] + SURFACE_HORIZONTAL_YARDS),
        );
        let mut candidates = self.fallback.clone();
        for x in low.0..=high.0 {
            for z in low.1..=high.1 {
                if let Some(indices) = self.cells.get(&(x, z)) {
                    candidates.extend_from_slice(indices);
                }
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        candidates
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub struct SurfaceIdentity {
    pub map_id: u32,
    pub tile: [i32; 3],
    pub polygon: usize,
}

/// Only NavTerrain flag meanings are classified. Bridges, interiors, and
/// transports require independent geometry or object evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceKind {
    Ground,
    Water,
    Magma,
    Slime,
    Mixed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct RouteSurface {
    pub identity: SurfaceIdentity,
    pub height: f32,
    pub kind: SurfaceKind,
    pub flags: u16,
    pub area: u8,
}

impl SurfaceKind {
    pub(super) fn from_flags(flags: u16) -> Self {
        // AzerothCore MapDefines.h: NAV_MAGMA=0x02, NAV_SLIME=0x04.
        match flags & 0x0f {
            NAV_GROUND => Self::Ground,
            NAV_WATER => Self::Water,
            0x02 => Self::Magma,
            0x04 => Self::Slime,
            0 => Self::Unknown,
            _ => Self::Mixed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SurfaceCandidate {
    pub identity: SurfaceIdentity,
    pub height: f32,
    pub horizontal_offset_yards: f32,
    pub flags: u16,
    pub area: u8,
    pub kind: SurfaceKind,
    pub raw_terrain_delta_yards: Option<f32>,
    /// None means unverified. Polyline proximity cannot prove connectivity.
    pub connected_to_route: Option<bool>,
    /// Lower ranks first. None means ineligible for this ground observation.
    pub observation_score_yards: Option<f32>,
}

#[derive(Debug, Clone, Copy)]
pub struct SurfaceProbe {
    pub point: [f32; 3],
    pub raw_terrain_height: Option<f32>,
    pub route_height: Option<f32>,
    pub previous: Option<SurfaceIdentity>,
}

/// Height on a nearby route segment, for observation ranking only. A route
/// polyline carries no polygon references and cannot prove connectivity.
pub fn nearby_route_height(point: [f32; 3], route: &[[f32; 3]]) -> Option<f32> {
    if !point.iter().all(|value| value.is_finite()) {
        return None;
    }
    route
        .windows(2)
        .filter_map(|segment| {
            let start = segment[0];
            let end = segment[1];
            if !start
                .iter()
                .chain(end.iter())
                .all(|value| value.is_finite())
            {
                return None;
            }
            let dx = end[0] - start[0];
            let dy = end[1] - start[1];
            let length = dx * dx + dy * dy;
            // Vertical segments and duplicate points provide no ground corridor.
            if !length.is_finite() || length <= f32::EPSILON {
                return None;
            }
            let fraction = (((point[0] - start[0]) * dx + (point[1] - start[1]) * dy) / length)
                .clamp(0.0, 1.0);
            let horizontal =
                (start[0] + fraction * dx - point[0]).hypot(start[1] + fraction * dy - point[1]);
            let height = start[2] + fraction * (end[2] - start[2]);
            let vertical = (height - point[2]).abs();
            (horizontal.is_finite()
                && height.is_finite()
                && horizontal <= 1.0
                && vertical <= LOCAL_SURFACE_VERTICAL_YARDS)
                .then_some((horizontal.hypot(vertical), height))
        })
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, height)| height)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceReference {
    WorkerPosition,
    NearbyRoute,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SurfaceObservation {
    pub point: [f32; 3],
    pub available: bool,
    pub raw_terrain_height: Option<f32>,
    pub reference_height: f32,
    pub reference_source: SurfaceReference,
    pub previous: Option<SurfaceIdentity>,
    pub selected: Option<SurfaceIdentity>,
    pub selection_changed: Option<bool>,
    pub candidates: Vec<SurfaceCandidate>,
    pub candidate_count: usize,
    pub truncated: bool,
    pub world_geometry_verified: bool,
}

impl SurfaceObservation {
    pub fn selected_candidate(&self) -> Option<&SurfaceCandidate> {
        let selected = self.selected?;
        self.candidates
            .iter()
            .find(|candidate| candidate.identity == selected)
    }
}

pub(super) fn observe_surface(
    map_id: u32,
    tile: Option<&DetourTile>,
    query: &SurfaceProbe,
) -> SurfaceObservation {
    let polygons = 0..tile.map_or(0, |tile| tile.polygons.len());
    observe_surface_polygons(map_id, tile, query, polygons)
}

pub(super) fn observe_indexed_surface(
    map_id: u32,
    tile: Option<&std::sync::Arc<IndexedTile>>,
    query: &SurfaceProbe,
) -> SurfaceObservation {
    let polygons = tile.map_or_else(Vec::new, |tile| tile.nearby(query.point));
    observe_surface_polygons(map_id, tile.map(|tile| &tile.tile), query, polygons)
}

fn observe_surface_polygons(
    map_id: u32,
    tile: Option<&DetourTile>,
    query: &SurfaceProbe,
    polygons: impl IntoIterator<Item = usize>,
) -> SurfaceObservation {
    let route_height = query.route_height.filter(|height| {
        height.is_finite() && (*height - query.point[2]).abs() <= LOCAL_SURFACE_VERTICAL_YARDS
    });
    let raw_terrain_height = query.raw_terrain_height.filter(|height| height.is_finite());
    let mut result = SurfaceObservation {
        point: query.point,
        available: false,
        raw_terrain_height,
        reference_height: route_height.unwrap_or(query.point[2]),
        reference_source: if route_height.is_some() {
            SurfaceReference::NearbyRoute
        } else {
            SurfaceReference::WorkerPosition
        },
        previous: query.previous.filter(|identity| identity.map_id == map_id),
        selected: None,
        selection_changed: None,
        candidates: Vec::with_capacity(MAX_SURFACE_CANDIDATES + 1),
        candidate_count: 0,
        truncated: false,
        world_geometry_verified: false,
    };
    let (Some(tile), Ok(point)) = (
        tile,
        DetourPoint::from_wow(query.point[0], query.point[1], query.point[2]),
    ) else {
        return result;
    };
    result.available = true;
    for polygon_index in polygons {
        let polygon = &tile.polygons[polygon_index];
        // Off-mesh links are actions, not supporting floor polygons.
        if polygon.polygon_type != 0 {
            continue;
        }
        let Some(closest) = tile.closest_point_on_polygon(polygon_index, point) else {
            continue;
        };
        let horizontal = (closest.x - point.x).hypot(closest.z - point.z);
        let vertical = (closest.y - point.y).abs();
        if !horizontal.is_finite()
            || !vertical.is_finite()
            || horizontal > SURFACE_HORIZONTAL_YARDS
            || vertical > SURFACE_SCAN_VERTICAL_YARDS
        {
            continue;
        }
        let identity = SurfaceIdentity {
            map_id,
            tile: [tile.header.x, tile.header.y, tile.header.layer],
            polygon: polygon_index,
        };
        let kind = SurfaceKind::from_flags(polygon.flags);
        let score =
            (kind == SurfaceKind::Ground && vertical <= LOCAL_SURFACE_VERTICAL_YARDS).then(|| {
                let continuity = if result.previous.is_some_and(|previous| previous != identity) {
                    CONTINUITY_PENALTY_YARDS
                } else {
                    0.0
                };
                (closest.y - result.reference_height).abs() + horizontal + continuity
            });
        result.candidate_count += 1;
        result.candidates.push(SurfaceCandidate {
            identity,
            height: closest.y,
            horizontal_offset_yards: horizontal,
            flags: polygon.flags,
            area: polygon.area,
            kind,
            raw_terrain_delta_yards: raw_terrain_height.map(|terrain| closest.y - terrain),
            connected_to_route: None,
            observation_score_yards: score,
        });
        // Keep memory bounded even when many polygons overlap at one point.
        result.candidates.sort_by(|left, right| {
            match (left.observation_score_yards, right.observation_score_yards) {
                (Some(left), Some(right)) => left.total_cmp(&right),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => (left.height - point.y)
                    .abs()
                    .total_cmp(&(right.height - point.y).abs()),
            }
            .then_with(|| left.identity.cmp(&right.identity))
        });
        result.candidates.truncate(MAX_SURFACE_CANDIDATES);
    }
    result.truncated = result.candidate_count > result.candidates.len();
    result.selected = result
        .candidates
        .first()
        .filter(|candidate| candidate.observation_score_yards.is_some())
        .map(|candidate| candidate.identity);
    result.selection_changed = result
        .previous
        .zip(result.selected)
        .map(|(old, new)| old != new);
    result
}
