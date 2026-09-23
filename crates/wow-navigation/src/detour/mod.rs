#![allow(
    dead_code,
    reason = "navigation queries are added in the next milestone"
)]

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;



use anyhow::{Context, Result, bail};


use reader::PacketReader;


mod reader;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerPositionSource { Authoritative }

#[derive(Clone, Debug)]
pub struct NavigationTuning {
    pub nearest_horizontal_yards: f32,
    pub nearest_vertical_yards: f32,
    pub destination_projection_horizontal_yards: f32,
    pub max_visited_nodes: usize,
    pub long_route_nodes_per_yard: usize,
    pub long_route_max_visited_nodes: usize,
    pub graph_cache_capacity: usize,
    pub terrain_sample_spacing_yards: f32,
    pub max_terrain_height_reconcile_yards: f32,
}
impl Default for NavigationTuning {
    fn default() -> Self { Self {
        nearest_horizontal_yards: 3.0, nearest_vertical_yards: 16.0,
        destination_projection_horizontal_yards: 12.0, max_visited_nodes: 4096,
        long_route_nodes_per_yard: 96, long_route_max_visited_nodes: 65536,
        graph_cache_capacity: 4, terrain_sample_spacing_yards: 1.0,
        max_terrain_height_reconcile_yards: 2.5,
    }}
}

fn lock_recover<'a, T>(mutex: &'a std::sync::Mutex<T>, _name: &'static str) -> std::sync::MutexGuard<'a, T> {
    match mutex.lock() { Ok(guard) => guard, Err(poisoned) => poisoned.into_inner() }
}

mod cache;
mod format;
mod graph;
mod route;
mod surface;

pub use cache::NavigationData;
pub use format::{
    DetailMesh, DetourPoint, DetourTile, DetourTileHeader, MmapTile, NavMeshParams, NavPolygon,
    OffMeshConnection, PolygonPoint,
};
pub use surface::{
    RouteSurface, SurfaceCandidate, SurfaceIdentity, SurfaceKind, SurfaceObservation, SurfaceProbe,
    SurfaceReference, nearby_route_height,
};

/// Semantic endpoint involved in a navigation projection failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationEndpoint {
    Start,
    Destination,
    IntermediateWaypoint,
    Position,
}

/// Typed failures produced by navigation orchestration and mmap route search.
///
/// Callers must classify behavior from these variants instead of parsing the
/// human-readable error text. The display strings remain diagnostic only.
#[derive(Clone, Debug, PartialEq)]
pub enum NavigationError {
    Cancelled,
    DeadlineExceeded {
        operation: &'static str,
        budget_ms: u64,
    },
    Disconnected {
        explored: Option<usize>,
    },
    NoNearbyPolygon {
        endpoint: NavigationEndpoint,
        detail: Option<String>,
    },
    SearchLimit {
        expanded_nodes: usize,
        limit: usize,
    },
    StartSurfaceMismatch {
        horizontal_gap: f32,
        vertical_gap: f32,
        position_source: Option<PlayerPositionSource>,
    },
    WorkerUnavailable {
        detail: String,
    },
    Superseded,
    Deferred,
    NoRoute {
        detail: String,
    },
}

impl std::fmt::Display for NavigationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("navigation route calculation was cancelled"),
            Self::DeadlineExceeded {
                operation,
                budget_ms,
            } => write!(
                f,
                "{operation} exceeded the {budget_ms} ms navigation time budget"
            ),
            Self::Disconnected { explored: Some(explored) } => write!(
                f,
                "navigation polygons are disconnected after exploring {explored} reachable polygons"
            ),
            Self::Disconnected { explored: None } => {
                f.write_str("navigation polygons are disconnected")
            }
            Self::NoNearbyPolygon { endpoint, detail } => {
                let endpoint = match endpoint {
                    NavigationEndpoint::Start => "start",
                    NavigationEndpoint::Destination => "destination",
                    NavigationEndpoint::IntermediateWaypoint => "intermediate waypoint",
                    NavigationEndpoint::Position => "position",
                };
                write!(f, "navigation {endpoint} has no nearby walkable polygon")?;
                if let Some(detail) = detail {
                    write!(f, "; {detail}")?;
                }
                Ok(())
            }
            Self::SearchLimit {
                expanded_nodes,
                limit,
            } => write!(
                f,
                "navigation path search exceeded its node limit after expanding {expanded_nodes} nodes with limit {limit}"
            ),
            Self::StartSurfaceMismatch {
                horizontal_gap,
                vertical_gap,
                position_source,
            } => write!(
                f,
                "navigation start is not attached to its mesh surface: horizontal offset {horizontal_gap:.3}, vertical offset {vertical_gap:.3}, position source {position_source:?}; authoritative position recovery is required"
            ),
            Self::WorkerUnavailable { detail } => write!(f, "navigation worker unavailable: {detail}"),
            Self::Superseded => f.write_str("navigation job result was superseded"),
            Self::Deferred => f.write_str(
                "voluntary route search deferred after matching route failures from the same origin",
            ),
            Self::NoRoute { detail } => write!(f, "no navigation route: {detail}"),
        }
    }
}

impl std::error::Error for NavigationError {}

/// Return the first typed navigation error in an anyhow error chain.
pub fn navigation_error(error: &anyhow::Error) -> Option<&NavigationError> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<NavigationError>())
}
