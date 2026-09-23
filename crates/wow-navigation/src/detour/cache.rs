//! Navigation cache responsibilities.

#[allow(unused_imports)]
use super::format::*;
#[allow(unused_imports)]
use super::graph::*;
#[allow(unused_imports)]
use super::route::*;
#[allow(unused_imports)]
use super::*;

use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(super) struct CachedTile {
    raw: MmapTile,
    parsed: OnceLock<std::result::Result<Arc<super::surface::IndexedTile>, String>>,
    identity: (u64, Option<std::time::SystemTime>),
}

impl CachedTile {
    fn parsed(&self) -> Result<Arc<super::surface::IndexedTile>> {
        self.parsed
            .get_or_init(|| {
                self.raw
                    .detour_tile()
                    .map(super::surface::IndexedTile::new)
                    .map(Arc::new)
                    .map_err(|error| format!("{error:#}"))
            })
            .clone()
            .map_err(anyhow::Error::msg)
    }
}

#[derive(Debug)]
pub(super) struct TileCache {
    pub(super) entries: HashMap<PathBuf, Arc<CachedTile>>,
    pub(super) order: VecDeque<PathBuf>,
    pub(super) capacity: usize,
}

impl TileCache {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    fn insert(&mut self, path: PathBuf, tile: Arc<CachedTile>) {
        self.order.retain(|known| known != &path);
        self.order.push_back(path.clone());
        self.entries.insert(path, tile);
        while self.entries.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }
}

const NEGATIVE_ROUTE_CACHE_CAPACITY: usize = 64;
const NEGATIVE_ROUTE_TTL: Duration = Duration::from_secs(10);

#[derive(Debug)]
struct CachedGraph {
    key: GraphKey,
    id: u64,
    graph: std::sync::Arc<RouteGraph>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NegativeRouteEndpointKey {
    bucket: [i32; 3],
    polygon: Option<PolygonRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NegativeRouteKey {
    graph_id: u64,
    start: NegativeRouteEndpointKey,
    end: NegativeRouteEndpointKey,
}

#[derive(Clone, Debug)]
struct NegativeRouteFailure {
    key: NegativeRouteKey,
    expires_at: Instant,
    error: NavigationError,
}

#[derive(Debug)]
pub struct NavigationData {
    pub(super) mmaps_dir: PathBuf,
    pub(super) cache: Mutex<TileCache>,
    pub(super) tuning: NavigationTuning,
    graphs: Mutex<VecDeque<CachedGraph>>,
    next_graph_id: AtomicU64,
    negative_routes: Mutex<VecDeque<NegativeRouteFailure>>,
}

// Ordered file identity matters: PolygonRef contains a tile vector index.
type GraphKey = (u32, bool, Vec<(String, u64, Option<std::time::SystemTime>)>);

fn negative_route_cacheable(error: &NavigationError) -> bool {
    matches!(
        error,
        NavigationError::Disconnected { .. }
            | NavigationError::NoNearbyPolygon { .. }
            | NavigationError::NoRoute { .. }
    )
}

fn route_point_bucket(point: (f32, f32, f32)) -> [i32; 3] {
    const XY_BUCKET_YARDS: f32 = 1.0;
    const Z_BUCKET_YARDS: f32 = 0.5;
    [
        (point.0 / XY_BUCKET_YARDS).floor() as i32,
        (point.1 / XY_BUCKET_YARDS).floor() as i32,
        (point.2 / Z_BUCKET_YARDS).floor() as i32,
    ]
}

fn negative_route_endpoint_key(
    point: DetourPoint,
    polygon: Option<PolygonRef>,
) -> NegativeRouteEndpointKey {
    NegativeRouteEndpointKey {
        bucket: route_point_bucket(point.to_wow()),
        polygon,
    }
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct LocalSurfaceSample {
    pub available: bool,
    pub height: Option<f32>,
    pub tile: Option<[i32; 3]>,
    pub polygon: Option<usize>,
}

impl NavigationData {
    /// Sample only route-loaded tiles. Missing or malformed tiles are unknown,
    /// not obstacles. This performs no route search or filesystem access.
    pub fn cached_surface_samples(
        &self,
        map_id: u32,
        points: &[[f32; 3]],
    ) -> Vec<LocalSurfaceSample> {
        let tiles = self.cached_sensor_tiles(map_id, points);
        points
            .iter()
            .map(|point| {
                let tile = wow_grid(point[0], point[1])
                    .ok()
                    .and_then(|grid| tiles.get(&grid))
                    .and_then(Option::as_ref);
                let probe = DetourPoint::from_wow(point[0], point[1], point[2]);
                match (tile, probe) {
                    (Some(tile), Ok(probe)) => {
                        let nearest = tile.nearest_polygon(probe, 0.05, 10.0, false);
                        LocalSurfaceSample {
                            available: true,
                            height: nearest.as_ref().map(|nearest| nearest.point.to_wow().2),
                            tile: Some([tile.header.x, tile.header.y, tile.header.layer]),
                            polygon: nearest.map(|nearest| nearest.polygon),
                        }
                    }
                    _ => LocalSurfaceSample {
                        available: false,
                        height: None,
                        tile: None,
                        polygon: None,
                    },
                }
            })
            .collect()
    }

    /// Attach the exact navmesh floor to accepted route points. Route points
    /// were projected onto these tiles during path construction, so Z is used
    /// to disambiguate bridges, basements, and stacked interiors. A route tile
    /// that was evicted from the small hot cache is loaded again explicitly.
    pub fn route_surfaces(
        &self,
        map_id: u32,
        points: &[[f32; 3]],
    ) -> Vec<Option<super::surface::RouteSurface>> {
        self.route_surfaces_cancellable(map_id, points, &|| false)
            .unwrap_or_else(|_| vec![None; points.len()])
    }

    pub fn route_surfaces_cancellable(
        &self,
        map_id: u32,
        points: &[[f32; 3]],
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<Option<super::surface::RouteSurface>>> {
        self.route_surfaces_bounded_cancellable(map_id, points, None, is_cancelled)
            .map(|(surfaces, _complete)| surfaces)
    }

    /// Attach route surfaces without allowing optional floor metadata to hold a
    /// validated route indefinitely. When `soft_budget` expires, the remaining
    /// entries are left as `None`; movement already treats missing route-surface
    /// metadata as a request to use the existing terrain/VMap fallback path.
    ///
    /// Dense route points usually stay on the same polygon for several samples.
    /// Reuse the previous polygon when it still contains the next point instead
    /// of running a full nearest-polygon query for every sub-yard waypoint.
    pub fn route_surfaces_bounded_cancellable(
        &self,
        map_id: u32,
        points: &[[f32; 3]],
        soft_budget: Option<Duration>,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<(Vec<Option<super::surface::RouteSurface>>, bool)> {
        if is_cancelled() {
            return Err(super::NavigationError::Cancelled.into());
        }
        let started = Instant::now();
        let tiles = self.route_sensor_tiles(map_id, points);
        let mut surfaces = Vec::with_capacity(points.len());
        let mut previous = None::<super::surface::RouteSurface>;
        let mut complete = true;

        for point in points {
            if is_cancelled() {
                return Err(super::NavigationError::Cancelled.into());
            }
            if soft_budget.is_some_and(|budget| started.elapsed() >= budget) {
                complete = false;
                break;
            }

            let surface = (|| {
                let grid = wow_grid(point[0], point[1]).ok()?;
                let tile = tiles.get(&grid)?.as_ref()?;
                let probe = DetourPoint::from_wow(point[0], point[1], point[2]).ok()?;
                let tile_identity = [tile.header.x, tile.header.y, tile.header.layer];

                if let Some(previous_surface) = previous
                    && previous_surface.identity.map_id == map_id
                    && previous_surface.identity.tile == tile_identity
                    && let Some(closest) =
                        tile.closest_point_on_polygon(previous_surface.identity.polygon, probe)
                {
                    let horizontal = (closest.x - probe.x).hypot(closest.z - probe.z);
                    let vertical = (closest.y - probe.y).abs();
                    if horizontal <= 0.10
                        && vertical <= 0.75
                        && let Some(polygon) = tile.polygons.get(previous_surface.identity.polygon)
                    {
                        return Some(super::surface::RouteSurface {
                            identity: previous_surface.identity,
                            height: closest.to_wow().2,
                            kind: super::surface::SurfaceKind::from_flags(polygon.flags),
                            flags: polygon.flags,
                            area: polygon.area,
                        });
                    }
                }

                let nearest = tile.nearest_polygon(probe, 0.10, 0.75, true)?;
                let polygon = tile.polygons.get(nearest.polygon)?;
                Some(super::surface::RouteSurface {
                    identity: super::surface::SurfaceIdentity {
                        map_id,
                        tile: tile_identity,
                        polygon: nearest.polygon,
                    },
                    height: nearest.point.to_wow().2,
                    kind: super::surface::SurfaceKind::from_flags(polygon.flags),
                    flags: polygon.flags,
                    area: polygon.area,
                })
            })();
            previous = surface;
            surfaces.push(surface);
        }

        surfaces.resize(points.len(), None);
        Ok((surfaces, complete))
    }

    /// Re-project a moving point onto one already-authorized route polygon.
    /// This preserves floor identity between sparse execution waypoints.
    pub fn height_on_route_surface(
        &self,
        surface: super::surface::RouteSurface,
        point: [f32; 3],
    ) -> Option<f32> {
        if !point.iter().all(|value| value.is_finite()) {
            return None;
        }
        let tiles = self.route_sensor_tiles(surface.identity.map_id, &[point]);
        let grid = wow_grid(point[0], point[1]).ok()?;
        let tile = tiles.get(&grid)?.as_ref()?;
        if [tile.header.x, tile.header.y, tile.header.layer] != surface.identity.tile {
            return None;
        }
        let probe = DetourPoint::from_wow(point[0], point[1], point[2]).ok()?;
        let closest = tile.closest_point_on_polygon(surface.identity.polygon, probe)?;
        let horizontal = (closest.x - probe.x).hypot(closest.z - probe.z);
        let vertical = (closest.y - probe.y).abs();
        (horizontal <= 0.75 && vertical <= 2.0).then_some(closest.to_wow().2)
    }

    /// Observe alternative surfaces and load the endpoint tile when needed.
    /// Ranking is advisory; route connectivity and independent collision must
    /// still authorize any correction before movement uses it.
    pub fn surface_candidates(
        &self,
        map_id: u32,
        probes: &[super::surface::SurfaceProbe],
    ) -> Vec<super::surface::SurfaceObservation> {
        let points: Vec<_> = probes.iter().map(|probe| probe.point).collect();
        let tiles = self.route_sensor_tiles(map_id, &points);
        probes
            .iter()
            .map(|probe| {
                let tile = wow_grid(probe.point[0], probe.point[1])
                    .ok()
                    .and_then(|grid| tiles.get(&grid))
                    .and_then(Option::as_ref);
                super::surface::observe_indexed_surface(map_id, tile, probe)
            })
            .collect()
    }

    /// Observe alternative surfaces in cached tiles. Ranking is advisory;
    /// neither route connectivity nor independent collision is inferred.
    pub fn cached_surface_candidates(
        &self,
        map_id: u32,
        probes: &[super::surface::SurfaceProbe],
    ) -> Vec<super::surface::SurfaceObservation> {
        let points: Vec<_> = probes.iter().map(|probe| probe.point).collect();
        let tiles = self.cached_sensor_tiles(map_id, &points);
        probes
            .iter()
            .map(|probe| {
                let tile = wow_grid(probe.point[0], probe.point[1])
                    .ok()
                    .and_then(|grid| tiles.get(&grid))
                    .and_then(Option::as_ref);
                super::surface::observe_indexed_surface(map_id, tile, probe)
            })
            .collect()
    }

    fn route_sensor_tiles(
        &self,
        map_id: u32,
        points: &[[f32; 3]],
    ) -> HashMap<(i32, i32), Option<Arc<super::surface::IndexedTile>>> {
        let mut grids = HashSet::new();
        for point in points {
            if let Ok(grid) = wow_grid(point[0], point[1]) {
                grids.insert(grid);
            }
        }
        grids
            .into_iter()
            .map(|grid @ (x, y)| {
                let path = self
                    .mmaps_dir
                    .join(format!("{map_id:03}{x:02}{y:02}.mmtile"));
                // Validate the file identity even when the tile is already
                // cached. The mmap directory can be rebuilt while the bot is
                // running; returning the old Arc here would keep stale
                // geometry in route and vision queries.
                let tile = self
                    .load_cached_tile(&path)
                    .ok()
                    .and_then(|tile| tile.parsed().ok());
                (grid, tile)
            })
            .collect()
    }

    fn cached_sensor_tiles(
        &self,
        map_id: u32,
        points: &[[f32; 3]],
    ) -> HashMap<(i32, i32), Option<Arc<super::surface::IndexedTile>>> {
        let mut grids = HashSet::new();
        for point in points {
            if let Ok(grid) = wow_grid(point[0], point[1]) {
                grids.insert(grid);
            }
        }
        // Validate file identity on sensor-cache hits as well as route-cache
        // hits. Parsing remains shared by CachedTile::parsed, so this adds a
        // metadata check without repeating geometry work.
        grids
            .into_iter()
            .map(|grid @ (x, y)| {
                let path = self
                    .mmaps_dir
                    .join(format!("{map_id:03}{x:02}{y:02}.mmtile"));
                let tile = self
                    .load_cached_tile(&path)
                    .ok()
                    .and_then(|tile| tile.parsed().ok());
                (grid, tile)
            })
            .collect()
    }

    pub fn new(mmaps_dir: impl Into<PathBuf>) -> Result<Self> {
        Self::new_with_tuning(mmaps_dir, NavigationTuning::default())
    }

    pub fn new_with_tuning(
        mmaps_dir: impl Into<PathBuf>,
        tuning: NavigationTuning,
    ) -> Result<Self> {
        let mmaps_dir = mmaps_dir.into();
        if !mmaps_dir.is_dir() {
            bail!(
                "movement-map directory does not exist: {}",
                mmaps_dir.display()
            );
        }
        if !tuning.nearest_horizontal_yards.is_finite()
            || !tuning.nearest_vertical_yards.is_finite()
            || !tuning.destination_projection_horizontal_yards.is_finite()
            || tuning.nearest_horizontal_yards <= 0.0
            || tuning.nearest_vertical_yards <= 0.0
            || tuning.destination_projection_horizontal_yards < tuning.nearest_horizontal_yards
            || tuning.max_visited_nodes == 0
            || tuning.long_route_nodes_per_yard == 0
            || tuning.long_route_max_visited_nodes < tuning.max_visited_nodes
            || tuning.graph_cache_capacity == 0
        {
            bail!("navigation tuning is invalid");
        }
        Ok(Self {
            mmaps_dir,
            cache: Mutex::new(TileCache::new(DEFAULT_CACHE_TILES)),
            graphs: Mutex::new(VecDeque::new()),
            next_graph_id: AtomicU64::new(1),
            negative_routes: Mutex::new(VecDeque::new()),
            tuning,
        })
    }

    pub fn map_params(&self, map_id: u32) -> Result<NavMeshParams> {
        let path = self.mmaps_dir.join(format!("{map_id:03}.mmap"));
        let bytes = fs::read(&path)
            .with_context(|| format!("failed to read mmap parameters from {}", path.display()))?;
        NavMeshParams::parse(&bytes)
    }

    pub fn load_tile_path(&self, path: &Path) -> Result<MmapTile> {
        Ok(self.load_cached_tile(path)?.raw.clone())
    }

    fn load_cached_tile(&self, path: &Path) -> Result<Arc<CachedTile>> {
        self.load_cached_tile_with_status(path)
            .map(|(tile, _cache_hit)| tile)
    }

    fn load_cached_tile_with_status(&self, path: &Path) -> Result<(Arc<CachedTile>, bool)> {
        if path.parent() != Some(self.mmaps_dir.as_path())
            || path.extension().and_then(|value| value.to_str()) != Some("mmtile")
        {
            bail!("movement-map tile path is outside the configured directory");
        }
        let metadata = fs::metadata(path)
            .with_context(|| format!("failed to inspect movement-map tile {}", path.display()))?;
        let identity = (metadata.len(), metadata.modified().ok());
        if let Some(tile) = lock_recover(&self.cache, "navigation.tile_cache")
            .entries
            .get(path)
            .filter(|tile| tile.identity == identity)
            .cloned()
        {
            return Ok((tile, true));
        }
        if metadata.len() > (MMAP_HEADER_SIZE + MAX_TILE_BYTES) as u64 {
            bail!("movement-map tile exceeds the input limit");
        }
        let tile =
            MmapTile::parse(&fs::read(path).with_context(|| {
                format!("failed to read movement-map tile {}", path.display())
            })?)?;
        let tile = Arc::new(CachedTile {
            raw: tile,
            parsed: OnceLock::new(),
            identity,
        });
        let mut cache = lock_recover(&self.cache, "navigation.tile_cache");
        // Concurrent cold loads must converge on one parse cell.
        if let Some(existing) = cache
            .entries
            .get(path)
            .filter(|entry| entry.identity == identity)
            .cloned()
        {
            return Ok((existing, true));
        }
        cache.insert(path.to_owned(), tile.clone());
        Ok((tile, false))
    }

    pub fn route(
        &self,
        map_id: u32,
        start: (f32, f32, f32),
        end: (f32, f32, f32),
        allow_water: bool,
    ) -> Result<Vec<[f32; 3]>> {
        self.route_cancellable(map_id, start, end, allow_water, &|| false)
    }

    /// Compute a route while allowing the owning orchestration job to stop a
    /// superseded search. The callback is checked between tile loads, graph
    /// construction batches, direct-route samples, and A* node expansions.
    pub fn route_cancellable(
        &self,
        map_id: u32,
        start: (f32, f32, f32),
        end: (f32, f32, f32),
        allow_water: bool,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Vec<[f32; 3]>> {
        ensure_route_not_cancelled(cancelled)?;
        let route_started = std::time::Instant::now();
        let start = DetourPoint::from_wow(start.0, start.1, start.2)?;
        if !end.0.is_finite() || !end.1.is_finite() || !end.2.is_finite() {
            bail!("navigation destination is not finite");
        }
        let end_probe = route_endpoint(end)?;
        let start_grid = wow_grid(start.to_wow().0, start.to_wow().1)?;
        let end_grid = wow_grid(end.0, end.1)?;
        let min_x = start_grid.0.min(end_grid.0).saturating_sub(1).max(0);
        let max_x = start_grid.0.max(end_grid.0).saturating_add(1).min(63);
        let min_y = start_grid.1.min(end_grid.1).saturating_sub(1).max(0);
        let max_y = start_grid.1.max(end_grid.1).saturating_add(1).min(63);
        let tile_count = usize::try_from(max_x - min_x + 1)?
            .checked_mul(usize::try_from(max_y - min_y + 1)?)
            .context("navigation tile window overflow")?;
        if tile_count > MAX_ROUTE_TILES {
            bail!("navigation route spans too many map tiles");
        }

        let params = self.map_params(map_id)?;
        let mut tiles = Vec::new();
        let mut loaded_tile_names = Vec::new();
        let mut loaded_tile_identity = Vec::new();
        let mut tile_cache_hits = 0usize;
        let mut tile_cache_misses = 0usize;
        let mut detour_coordinates = HashSet::new();
        for grid_x in min_x..=max_x {
            for grid_y in min_y..=max_y {
                ensure_route_not_cancelled(cancelled)?;
                // AzerothCore mmap tile filenames use mapId + gridX + gridY.
                // Keep this order aligned with wow_grid(); swapping the axes loads
                // geographically distant tiles even though their Detour payloads parse.
                let path = self
                    .mmaps_dir
                    .join(format!("{map_id:03}{grid_x:02}{grid_y:02}.mmtile"));
                if !path.is_file() {
                    continue;
                }
                let (cached_tile, tile_cache_hit) = self.load_cached_tile_with_status(&path)?;
                if tile_cache_hit {
                    tile_cache_hits = tile_cache_hits.saturating_add(1);
                } else {
                    tile_cache_misses = tile_cache_misses.saturating_add(1);
                }
                let tile = cached_tile.parsed()?.tile.clone();
                validate_tile_coordinates(&params, &tile)?;
                let tile_name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("<non-utf8>")
                    .to_owned();
                loaded_tile_names.push(tile_name.clone());
                loaded_tile_identity.push((
                    tile_name,
                    cached_tile.identity.0,
                    cached_tile.identity.1,
                ));
                if !detour_coordinates.insert((tile.header.x, tile.header.y, tile.header.layer)) {
                    bail!("movement-map tile coordinates are duplicated");
                }
                tiles.push(tile);
            }
        }
        if tiles.is_empty() {
            bail!("no movement-map tiles are available for the route");
        }
        let start_wow = start.to_wow();
        let straight_line_yards = (end.0 - start_wow.0).hypot(end.1 - start_wow.1);
        let distance_budget = (straight_line_yards.ceil() as usize)
            .saturating_mul(self.tuning.long_route_nodes_per_yard);
        let search_node_budget = self
            .tuning
            .max_visited_nodes
            .max(distance_budget)
            .min(self.tuning.long_route_max_visited_nodes);
        tracing::debug!(
            map_id,
            wow_start_x = start.to_wow().0,
            wow_start_y = start.to_wow().1,
            wow_start_z = start.to_wow().2,
            wow_end_x = end.0,
            wow_end_y = end.1,
            wow_end_z = end.2,
            start_grid_x = start_grid.0,
            start_grid_y = start_grid.1,
            end_grid_x = end_grid.0,
            end_grid_y = end_grid.1,
            loaded_tiles = tiles.len(),
            tile_cache_hit = tile_cache_hits,
            tile_cache_miss = tile_cache_misses,
            metadata_calls_per_route = tile_cache_hits.saturating_add(tile_cache_misses),
            tile_names = ?loaded_tile_names,
            nearest_horizontal = self.tuning.nearest_horizontal_yards,
            nearest_vertical = self.tuning.nearest_vertical_yards,
            allow_water,
            straight_line_yards,
            search_node_budget,
            "navigation route probe"
        );
        let graph_started = std::time::Instant::now();
        tracing::debug!(
            map_id,
            tile_load_ms = route_started.elapsed().as_millis() as u64,
            loaded_tiles = tiles.len(),
            tile_cache_hit = tile_cache_hits,
            tile_cache_miss = tile_cache_misses,
            metadata_calls_per_route = tile_cache_hits.saturating_add(tile_cache_misses),
            "navigation tiles ready"
        );
        let key = (map_id, allow_water, loaded_tile_identity);
        let cached = {
            let mut graphs = lock_recover(&self.graphs, "navigation graphs");
            graphs
                .iter()
                .position(|known| known.key == key)
                .map(|index| {
                    let entry = graphs.remove(index).expect("existing graph");
                    let id = entry.id;
                    let graph = entry.graph.clone();
                    graphs.push_back(entry);
                    (id, graph)
                })
        };
        let graph_cache_hit = cached.is_some();
        let (graph_id, graph) = match cached {
            Some(cached) => cached,
            None => {
                let graph = std::sync::Arc::new(build_graph(&tiles, allow_water, cancelled)?);
                ensure_route_not_cancelled(cancelled)?;
                let id = self.next_graph_id.fetch_add(1, AtomicOrdering::Relaxed);
                let mut graphs = lock_recover(&self.graphs, "navigation graphs");
                graphs.push_back(CachedGraph {
                    key,
                    id,
                    graph: graph.clone(),
                });
                while graphs.len() > self.tuning.graph_cache_capacity {
                    graphs.pop_front();
                }
                (id, graph)
            }
        };
        tracing::debug!(
            map_id,
            graph_cache_hit,
            graph_cache_miss = !graph_cache_hit,
            graph_cache_capacity = self.tuning.graph_cache_capacity,
            graph_builds = if graph_cache_hit { 0usize } else { 1usize },
            graph_ms = graph_started.elapsed().as_millis() as u64,
            polygons = graph.len(),
            components = graph.component_count(),
            "navigation graph ready"
        );
        let start_polygon = nearest_loaded_polygon(&tiles, start, allow_water, &self.tuning)
            .map(|(polygon, _)| polygon);
        let end_polygon = nearest_loaded_polygon(&tiles, end_probe, allow_water, &self.tuning)
            .map(|(polygon, _)| polygon);
        let negative_key = NegativeRouteKey {
            graph_id,
            start: negative_route_endpoint_key(start, start_polygon),
            end: negative_route_endpoint_key(end_probe, end_polygon),
        };
        if let Some(error) = self.cached_negative_route(negative_key) {
            tracing::debug!(
                map_id,
                graph_id,
                negative_cache_hit = true,
                error = %error,
                "reusing short-lived deterministic navigation failure"
            );
            return Err(error.into());
        }
        let search_started = std::time::Instant::now();
        let result = route_loaded_tiles_with_graph(
            &tiles,
            start,
            end_probe,
            allow_water,
            search_node_budget,
            &self.tuning,
            cancelled,
            Some(graph.as_ref()),
        );
        if let Err(error) = &result
            && let Some(error) = navigation_error(error)
            && negative_route_cacheable(error)
        {
            self.record_negative_route(negative_key, error.clone());
        }
        tracing::debug!(
            map_id,
            negative_cache_hit = false,
            negative_cache_miss = true,
            a_star_searches = 1usize,
            route_stage_duration_ms = search_started.elapsed().as_millis() as u64,
            search_ms = search_started.elapsed().as_millis() as u64,
            total_ms = route_started.elapsed().as_millis() as u64,
            success = result.is_ok(),
            "navigation search finished"
        );
        result
            .map_err(|error| {
                tracing::warn!(
                    map_id,
                    wow_start_x = start.to_wow().0,
                    wow_start_y = start.to_wow().1,
                    wow_start_z = start.to_wow().2,
                    wow_end_x = end.0,
                    wow_end_y = end.1,
                    wow_end_z = end.2,
                    start_grid_x = start_grid.0,
                    start_grid_y = start_grid.1,
                    end_grid_x = end_grid.0,
                    end_grid_y = end_grid.1,
                    loaded_tiles = tiles.len(),
                    tile_names = ?loaded_tile_names,
                    nearest_horizontal = self.tuning.nearest_horizontal_yards,
                    nearest_vertical = self.tuning.nearest_vertical_yards,
                    allow_water,
                    straight_line_yards,
                    search_node_budget,
                    error = %error,
                    "navigation route failed"
                );
                error
            })
            .map(|points| points.into_iter().map(DetourPoint::to_wow_array).collect())
    }

    fn cached_negative_route(&self, key: NegativeRouteKey) -> Option<NavigationError> {
        let now = Instant::now();
        let mut failures = lock_recover(&self.negative_routes, "navigation negative routes");
        failures.retain(|failure| failure.expires_at > now);
        failures
            .iter()
            .rev()
            .find(|failure| failure.key == key)
            .map(|failure| failure.error.clone())
    }

    fn record_negative_route(&self, key: NegativeRouteKey, error: NavigationError) {
        if !negative_route_cacheable(&error) {
            return;
        }
        let now = Instant::now();
        let mut failures = lock_recover(&self.negative_routes, "navigation negative routes");
        failures.retain(|failure| failure.expires_at > now && failure.key != key);
        failures.push_back(NegativeRouteFailure {
            key,
            expires_at: now + NEGATIVE_ROUTE_TTL,
            error,
        });
        while failures.len() > NEGATIVE_ROUTE_CACHE_CAPACITY {
            failures.pop_front();
        }
    }

    /// Project one semantic destination onto a nearby walkable mmap surface.
    /// This validates generated points, such as corpse-reclaim candidates,
    /// before another subsystem stores or acts on them.
    pub fn project_walkable_position(
        &self,
        map_id: u32,
        point: (f32, f32, f32),
        allow_water: bool,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<[f32; 3]> {
        ensure_route_not_cancelled(cancelled)?;
        let probe = route_endpoint(point)?;
        let (grid_x, grid_y) = wow_grid(point.0, point.1)?;
        let params = self.map_params(map_id)?;
        let mut tiles = Vec::new();
        let mut coordinates = HashSet::new();
        for x in grid_x.saturating_sub(1).max(0)..=grid_x.saturating_add(1).min(63) {
            for y in grid_y.saturating_sub(1).max(0)..=grid_y.saturating_add(1).min(63) {
                ensure_route_not_cancelled(cancelled)?;
                let path = self
                    .mmaps_dir
                    .join(format!("{map_id:03}{x:02}{y:02}.mmtile"));
                if !path.is_file() {
                    continue;
                }
                let tile = self.load_cached_tile(&path)?.parsed()?.tile.clone();
                validate_tile_coordinates(&params, &tile)?;
                if !coordinates.insert((tile.header.x, tile.header.y, tile.header.layer)) {
                    bail!("movement-map tile coordinates are duplicated");
                }
                tiles.push(tile);
            }
        }
        if tiles.is_empty() {
            bail!("no movement-map tiles are available near the requested position");
        }
        let projected = nearest_loaded_polygon_with_extents(
            &tiles,
            probe,
            self.tuning.destination_projection_horizontal_yards,
            self.tuning.nearest_vertical_yards,
            allow_water,
        )
        .ok_or_else(|| {
            anyhow::Error::from(NavigationError::NoNearbyPolygon {
                endpoint: NavigationEndpoint::Position,
                detail: None,
            })
        })?;
        ensure_route_not_cancelled(cancelled)?;
        Ok(projected.1.to_wow_array())
    }
}
