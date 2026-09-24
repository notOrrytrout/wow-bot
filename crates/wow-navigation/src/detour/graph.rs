//! Navigation graph responsibilities.

#[allow(unused_imports)]
use super::cache::*;
#[allow(unused_imports)]
use super::format::*;
#[allow(unused_imports)]
use super::route::*;
#[allow(unused_imports)]
use super::*;

#[derive(Debug)]
pub(super) struct RouteGraph {
    edges: HashMap<PolygonRef, Vec<GraphEdge>>,
    components: HashMap<PolygonRef, u32>,
    component_count: u32,
}

impl RouteGraph {
    fn new(edges: HashMap<PolygonRef, Vec<GraphEdge>>) -> Self {
        let (components, component_count) = graph_components(&edges);
        Self {
            edges,
            components,
            component_count,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.edges.len()
    }

    pub(super) fn component_count(&self) -> u32 {
        self.component_count
    }

    pub(super) fn same_component(&self, left: PolygonRef, right: PolygonRef) -> bool {
        matches!(
            (self.components.get(&left), self.components.get(&right)),
            (Some(left), Some(right)) if left == right
        )
    }

    pub(super) fn edges(&self) -> &HashMap<PolygonRef, Vec<GraphEdge>> {
        &self.edges
    }
}

fn graph_components(
    graph: &HashMap<PolygonRef, Vec<GraphEdge>>,
) -> (HashMap<PolygonRef, u32>, u32) {
    // Use weak connectivity. Ground navigation edges are expected to be
    // symmetric, but treating each edge as undirected keeps this precheck
    // conservative if malformed data contains a one-way adjacency. Different
    // weak components can never have a directed route between them.
    let mut adjacency = HashMap::<PolygonRef, Vec<PolygonRef>>::new();
    for (&from, edges) in graph {
        adjacency.entry(from).or_default();
        for edge in edges {
            adjacency.entry(from).or_default().push(edge.to);
            adjacency.entry(edge.to).or_default().push(from);
        }
    }

    let mut components = HashMap::with_capacity(adjacency.len());
    let mut component = 0_u32;
    for &start in adjacency.keys() {
        if components.contains_key(&start) {
            continue;
        }
        component = component.saturating_add(1);
        let mut pending = VecDeque::from([start]);
        components.insert(start, component);
        while let Some(node) = pending.pop_front() {
            for &next in adjacency.get(&node).into_iter().flatten() {
                if components.insert(next, component).is_none() {
                    pending.push_back(next);
                }
            }
        }
    }
    (components, component)
}

pub(super) fn nearest_loaded_polygon(
    tiles: &[DetourTile],
    point: DetourPoint,
    allow_water: bool,
    tuning: &NavigationTuning,
) -> Option<(PolygonRef, DetourPoint)> {
    nearest_loaded_polygon_in_extent(
        tiles,
        point,
        tuning.nearest_horizontal_yards,
        tuning.nearest_vertical_yards,
        allow_water,
    )
}

/// Return several nearby walkable polygons, ordered by projection distance.
/// The closest polygons can belong to isolated mesh islands, so callers can
/// select a connected candidate pair instead of failing on the first match.
pub(super) fn nearest_loaded_polygons(
    tiles: &[DetourTile],
    point: DetourPoint,
    horizontal: f32,
    vertical: f32,
    allow_water: bool,
    limit: usize,
) -> Vec<(PolygonRef, DetourPoint)> {
    let mut candidates = tiles
        .iter()
        .enumerate()
        .flat_map(|(tile_index, tile)| {
            tile.polygons
                .iter()
                .enumerate()
                .filter_map(move |(polygon_index, polygon)| {
                    if !polygon.walkable(allow_water) {
                        return None;
                    }
                    let projected = tile.closest_point_on_polygon(polygon_index, point)?;
                    let dx = projected.x - point.x;
                    let dz = projected.z - point.z;
                    let dy = (projected.y - point.y).abs();
                    (dx.hypot(dz) <= horizontal && dy <= vertical).then_some((
                        distance3(point, projected),
                        PolygonRef {
                            tile: tile_index,
                            polygon: polygon_index,
                        },
                        projected,
                    ))
                })
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.0.total_cmp(&right.0));
    candidates.dedup_by(|left, right| left.1 == right.1);
    candidates.truncate(limit);
    candidates
        .into_iter()
        .map(|(_, polygon, point)| (polygon, point))
        .collect()
}

pub(super) fn build_graph(
    tiles: &[DetourTile],
    allow_water: bool,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<RouteGraph> {
    let mut graph = HashMap::<PolygonRef, Vec<GraphEdge>>::new();
    let mut external = Vec::<(PolygonRef, [DetourPoint; 2])>::new();
    for (tile_index, tile) in tiles.iter().enumerate() {
        ensure_route_not_cancelled(cancelled)?;
        for (polygon_index, polygon) in tile.polygons.iter().enumerate() {
            if polygon_index % 256 == 0 {
                ensure_route_not_cancelled(cancelled)?;
            }
            if !polygon.walkable(allow_water) {
                continue;
            }
            let from = PolygonRef {
                tile: tile_index,
                polygon: polygon_index,
            };
            graph.entry(from).or_default();
            for (edge_index, neighbor) in polygon.neighbors.iter().copied().enumerate() {
                if neighbor == 0 {
                    continue;
                }
                let portal = polygon_edge(tile, polygon_index, edge_index)?;
                if neighbor & EXTERNAL_LINK != 0 {
                    external.push((from, portal));
                } else if let Some(neighbor) = usize::from(neighbor).checked_sub(1)
                    && tile
                        .polygons
                        .get(neighbor)
                        .is_some_and(|polygon| polygon.walkable(allow_water))
                {
                    graph.entry(from).or_default().push(GraphEdge {
                        to: PolygonRef {
                            tile: tile_index,
                            polygon: neighbor,
                        },
                        portal,
                    });
                }
            }
        }
    }
    // External portal stitching used to compare every external edge against
    // every other external edge, even though only immediately adjacent tiles
    // can connect.  Group edges by tile first so route construction does not
    // spend O(all_external_edges^2) CPU on large semantic travel queries.
    let mut external_by_tile = vec![Vec::<(PolygonRef, [DetourPoint; 2])>::new(); tiles.len()];
    for entry in external.iter().copied() {
        external_by_tile[entry.0.tile].push(entry);
    }

    let mut stitched_external_portals = 0usize;
    let mut compared_external_portal_pairs = 0usize;
    for left_tile_index in 0..tiles.len() {
        ensure_route_not_cancelled(cancelled)?;
        for right_tile_index in (left_tile_index + 1)..tiles.len() {
            let left_tile = &tiles[left_tile_index];
            let right_tile = &tiles[right_tile_index];
            let tile_dx = (left_tile.header.x - right_tile.header.x).abs();
            let tile_dy = (left_tile.header.y - right_tile.header.y).abs();
            if tile_dx + tile_dy != 1 {
                continue;
            }
            let vertical_tolerance = left_tile
                .header
                .walkable_climb
                .max(right_tile.header.walkable_climb)
                + PORTAL_EPSILON;
            for left in &external_by_tile[left_tile_index] {
                for right in &external_by_tile[right_tile_index] {
                    if compared_external_portal_pairs.is_multiple_of(512) {
                        ensure_route_not_cancelled(cancelled)?;
                    }
                    compared_external_portal_pairs += 1;
                    let Some(portal) =
                        overlapping_external_portal(left.1, right.1, vertical_tolerance)
                    else {
                        continue;
                    };
                    graph.entry(left.0).or_default().push(GraphEdge {
                        to: right.0,
                        portal,
                    });
                    graph
                        .entry(right.0)
                        .or_default()
                        .push(GraphEdge { to: left.0, portal });
                    stitched_external_portals += 1;
                }
            }
        }
    }
    tracing::debug!(
        external_portals = external.len(),
        compared_external_portal_pairs,
        stitched_external_portals,
        "stitched external navigation portals across loaded mmap tiles"
    );
    // Off-mesh connections can encode jumps, drops, teleports, or other
    // traversal semantics.  This executor currently consumes only plain XYZ
    // corridors, so using those links would erase the transition type and
    // could make a non-walkable transition look like ordinary ground travel.
    // Keep routing to connected walkable polygons until off-mesh actions are
    // represented explicitly in the movement plan.
    Ok(RouteGraph::new(graph))
}

pub(super) fn add_off_mesh_edges(
    tiles: &[DetourTile],
    allow_water: bool,
    graph: &mut HashMap<PolygonRef, Vec<GraphEdge>>,
    tuning: &NavigationTuning,
) -> Result<()> {
    for tile in tiles {
        for connection in &tile.off_mesh_connections {
            let start = nearest_loaded_polygon_with_extents(
                tiles,
                connection.start,
                connection.radius.max(PORTAL_EPSILON),
                tuning.nearest_vertical_yards,
                allow_water,
            );
            let end = nearest_loaded_polygon_with_extents(
                tiles,
                connection.end,
                connection.radius.max(PORTAL_EPSILON),
                tuning.nearest_vertical_yards,
                allow_water,
            );
            let (Some(start), Some(end)) = (start, end) else {
                continue;
            };
            graph.entry(start.0).or_default().push(GraphEdge {
                to: end.0,
                portal: [connection.start, connection.start],
            });
            if connection.bidirectional {
                graph.entry(end.0).or_default().push(GraphEdge {
                    to: start.0,
                    portal: [connection.end, connection.end],
                });
            }
        }
    }
    Ok(())
}

pub(super) fn nearest_loaded_polygon_with_extents(
    tiles: &[DetourTile],
    point: DetourPoint,
    horizontal: f32,
    vertical: f32,
    allow_water: bool,
) -> Option<(PolygonRef, DetourPoint)> {
    nearest_loaded_polygon_in_extent(tiles, point, horizontal, vertical, allow_water)
}

fn nearest_loaded_polygon_in_extent(
    tiles: &[DetourTile],
    point: DetourPoint,
    horizontal: f32,
    vertical: f32,
    allow_water: bool,
) -> Option<(PolygonRef, DetourPoint)> {
    tiles
        .iter()
        .enumerate()
        .filter_map(|(tile_index, tile)| {
            tile.nearest_polygon(point, horizontal, vertical, allow_water)
                .map(|nearest| {
                    (
                        distance3(point, nearest.point),
                        PolygonRef {
                            tile: tile_index,
                            polygon: nearest.polygon,
                        },
                        nearest.point,
                    )
                })
        })
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, polygon, point)| (polygon, point))
}

#[derive(Debug)]
struct PathSearchLimit {
    expanded_nodes: usize,
    limit: usize,
}

impl std::fmt::Display for PathSearchLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "navigation path search exceeded its node limit after expanding {} nodes with limit {}",
            self.expanded_nodes, self.limit
        )
    }
}
impl std::error::Error for PathSearchLimit {}

fn navigation_search_limit(error: anyhow::Error) -> anyhow::Error {
    match error.downcast::<PathSearchLimit>() {
        Ok(limit) => NavigationError::SearchLimit {
            expanded_nodes: limit.expanded_nodes,
            limit: limit.limit,
        }
        .into(),
        Err(error) => error,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn find_loaded_path_with_retry(
    tiles: &[DetourTile],
    graph: &RouteGraph,
    start: PolygonRef,
    end: PolygonRef,
    initial_limit: usize,
    maximum_limit: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<PolygonRef>> {
    if !graph.same_component(start, end) {
        return Err(NavigationError::Disconnected { explored: None }.into());
    }
    let result = find_loaded_path(tiles, graph.edges(), start, end, initial_limit, cancelled);
    let expanded_limit = maximum_limit.min(graph.len());
    if result
        .as_ref()
        .is_err_and(|error| error.is::<PathSearchLimit>())
        && expanded_limit > initial_limit
    {
        // A short geometric distance can still require a large detour. Retry
        // only budget exhaustion, within the configured hard cap.
        ensure_route_not_cancelled(cancelled)?;
        return find_loaded_path(tiles, graph.edges(), start, end, expanded_limit, cancelled)
            .map_err(navigation_search_limit);
    }
    result.map_err(navigation_search_limit)
}

pub(super) fn find_loaded_path(
    tiles: &[DetourTile],
    graph: &HashMap<PolygonRef, Vec<GraphEdge>>,
    start: PolygonRef,
    end: PolygonRef,
    max_visited: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<PolygonRef>> {
    let limit = max_visited.max(1);
    let end_center = loaded_polygon_center(tiles, end)?;
    let mut open = BinaryHeap::new();
    let mut costs = HashMap::new();
    let mut previous = HashMap::new();
    let mut closed = HashSet::new();
    costs.insert(start, 0.0f32);
    open.push(GraphOpenNode {
        polygon: start,
        estimated_total: distance3(loaded_polygon_center(tiles, start)?, end_center),
    });
    while let Some(node) = open.pop() {
        ensure_route_not_cancelled(cancelled)?;
        if !closed.insert(node.polygon) {
            continue;
        }
        if closed.len() > limit {
            return Err(PathSearchLimit {
                expanded_nodes: closed.len(),
                limit,
            }
            .into());
        }
        if node.polygon == end {
            let mut path = vec![end];
            let mut current = end;
            while current != start {
                current = *previous
                    .get(&current)
                    .context("navigation path predecessor is missing")?;
                path.push(current);
            }
            path.reverse();
            return Ok(path);
        }
        let current_center = loaded_polygon_center(tiles, node.polygon)?;
        for edge in graph.get(&node.polygon).into_iter().flatten() {
            if closed.contains(&edge.to) {
                continue;
            }
            let neighbor_center = loaded_polygon_center(tiles, edge.to)?;
            let next = costs[&node.polygon]
                + terrain_aware_transition_cost(current_center, neighbor_center);
            if next < *costs.get(&edge.to).unwrap_or(&f32::INFINITY) {
                costs.insert(edge.to, next);
                previous.insert(edge.to, node.polygon);
                open.push(GraphOpenNode {
                    polygon: edge.to,
                    estimated_total: next + distance3(neighbor_center, end_center),
                });
            }
        }
    }
    Err(NavigationError::Disconnected {
        explored: Some(closed.len()),
    }
    .into())
}

/// Prefer level ground when two navmesh corridors are otherwise viable. Mmaps
/// mark polygons as walkable, but they do not encode a preference for roads or
/// valleys. Keep steep ground available for necessary climbs while making it
/// progressively more expensive than a flatter detour.
fn terrain_aware_transition_cost(from: DetourPoint, to: DetourPoint) -> f32 {
    let horizontal = (to.x - from.x).hypot(to.z - from.z).max(0.01);
    let distance = distance3(from, to);
    let slope_degrees = ((to.y - from.y).abs() / horizontal).atan().to_degrees();
    let excess = (slope_degrees - 12.0).max(0.0);
    distance * (1.0 + excess * excess * 0.02)
}

pub(super) fn loaded_polygon_center(
    tiles: &[DetourTile],
    polygon: PolygonRef,
) -> Result<DetourPoint> {
    tiles
        .get(polygon.tile)
        .context("navigation tile is unavailable")?
        .polygon_center(polygon.polygon)
}

pub(super) fn polygon_edge(
    tile: &DetourTile,
    polygon: usize,
    edge: usize,
) -> Result<[DetourPoint; 2]> {
    let points = tile
        .polygon_points(polygon)
        .context("navigation polygon edge is unavailable")?;
    if edge >= points.len() {
        bail!("navigation polygon edge index is invalid");
    }
    Ok([points[edge], points[(edge + 1) % points.len()]])
}

pub(super) fn same_portal(left: [DetourPoint; 2], right: [DetourPoint; 2]) -> bool {
    (near_point(left[0], right[0]) && near_point(left[1], right[1]))
        || (near_point(left[0], right[1]) && near_point(left[1], right[0]))
}

// Detour can split the same tile-border portal into different polygon edge
// segments on each side. Requiring identical endpoints disconnects otherwise
// continuous navmesh islands. External tile portals are axis-aligned in the
// horizontal X/Z plane, so connect the actual overlapping segment instead.
pub(super) fn overlapping_external_portal(
    left: [DetourPoint; 2],
    right: [DetourPoint; 2],
    vertical_tolerance: f32,
) -> Option<[DetourPoint; 2]> {
    if same_portal(left, right) {
        return Some(left);
    }

    let left_x_constant = (left[0].x - left[1].x).abs() <= PORTAL_EPSILON;
    let right_x_constant = (right[0].x - right[1].x).abs() <= PORTAL_EPSILON;
    if left_x_constant && right_x_constant && (left[0].x - right[0].x).abs() <= PORTAL_EPSILON {
        let overlap_min = left[0].z.min(left[1].z).max(right[0].z.min(right[1].z));
        let overlap_max = left[0].z.max(left[1].z).min(right[0].z.max(right[1].z));
        if overlap_max - overlap_min > PORTAL_EPSILON {
            return portal_overlap_points(
                left,
                right,
                true,
                overlap_min,
                overlap_max,
                vertical_tolerance,
            );
        }
    }

    let left_z_constant = (left[0].z - left[1].z).abs() <= PORTAL_EPSILON;
    let right_z_constant = (right[0].z - right[1].z).abs() <= PORTAL_EPSILON;
    if left_z_constant && right_z_constant && (left[0].z - right[0].z).abs() <= PORTAL_EPSILON {
        let overlap_min = left[0].x.min(left[1].x).max(right[0].x.min(right[1].x));
        let overlap_max = left[0].x.max(left[1].x).min(right[0].x.max(right[1].x));
        if overlap_max - overlap_min > PORTAL_EPSILON {
            return portal_overlap_points(
                left,
                right,
                false,
                overlap_min,
                overlap_max,
                vertical_tolerance,
            );
        }
    }

    None
}

pub(super) fn portal_overlap_points(
    left: [DetourPoint; 2],
    right: [DetourPoint; 2],
    constant_x: bool,
    overlap_min: f32,
    overlap_max: f32,
    vertical_tolerance: f32,
) -> Option<[DetourPoint; 2]> {
    let point_at = |segment: [DetourPoint; 2], coordinate: f32| {
        let (start, end) = if constant_x {
            (segment[0].z, segment[1].z)
        } else {
            (segment[0].x, segment[1].x)
        };
        let span = end - start;
        let ratio = if span.abs() <= PORTAL_EPSILON {
            0.0
        } else {
            ((coordinate - start) / span).clamp(0.0, 1.0)
        };
        DetourPoint {
            x: segment[0].x + (segment[1].x - segment[0].x) * ratio,
            y: segment[0].y + (segment[1].y - segment[0].y) * ratio,
            z: segment[0].z + (segment[1].z - segment[0].z) * ratio,
        }
    };

    let make_point = |coordinate: f32| {
        let left_point = point_at(left, coordinate);
        let right_point = point_at(right, coordinate);
        if (left_point.y - right_point.y).abs() > vertical_tolerance {
            return None;
        }
        Some(DetourPoint {
            x: (left_point.x + right_point.x) * 0.5,
            y: (left_point.y + right_point.y) * 0.5,
            z: (left_point.z + right_point.z) * 0.5,
        })
    };

    Some([make_point(overlap_min)?, make_point(overlap_max)?])
}

pub(super) fn near_point(left: DetourPoint, right: DetourPoint) -> bool {
    (left.x - right.x).abs() <= PORTAL_EPSILON
        && (left.y - right.y).abs() <= PORTAL_EPSILON
        && (left.z - right.z).abs() <= PORTAL_EPSILON
}

pub(super) fn oriented_portal(
    tiles: &[DetourTile],
    from: PolygonRef,
    to: PolygonRef,
    portal: [DetourPoint; 2],
) -> Result<[DetourPoint; 2]> {
    if near_point(portal[0], portal[1]) {
        return Ok(portal);
    }
    let from = loaded_polygon_center(tiles, from)?;
    let to = loaded_polygon_center(tiles, to)?;
    let middle = DetourPoint {
        x: (portal[0].x + portal[1].x) * 0.5,
        y: (portal[0].y + portal[1].y) * 0.5,
        z: (portal[0].z + portal[1].z) * 0.5,
    };
    let cross =
        (to.x - from.x) * (portal[0].z - middle.z) - (to.z - from.z) * (portal[0].x - middle.x);
    Ok(if cross >= 0.0 {
        portal
    } else {
        [portal[1], portal[0]]
    })
}

pub(super) fn string_pull(portals: &[[DetourPoint; 2]]) -> Vec<DetourPoint> {
    if portals.is_empty() {
        return Vec::new();
    }
    let mut points = vec![portals[0][0]];
    let mut apex = portals[0][0];
    let mut left = apex;
    let mut right = apex;
    let mut left_index = 0usize;
    let mut right_index = 0usize;
    let mut index = 1usize;
    while index < portals.len() {
        let new_left = portals[index][0];
        let new_right = portals[index][1];
        if triangle_area_xz(apex, right, new_right) <= 0.0 {
            if near_point(apex, right) || triangle_area_xz(apex, left, new_right) > 0.0 {
                right = new_right;
                right_index = index;
            } else {
                points.push(left);
                apex = left;
                let next_index = left_index + 1;
                left = apex;
                right = apex;
                right_index = left_index;
                index = next_index;
                continue;
            }
        }
        if triangle_area_xz(apex, left, new_left) >= 0.0 {
            if near_point(apex, left) || triangle_area_xz(apex, right, new_left) < 0.0 {
                left = new_left;
                left_index = index;
            } else {
                points.push(right);
                apex = right;
                let next_index = right_index + 1;
                left = apex;
                right = apex;
                left_index = right_index;
                index = next_index;
                continue;
            }
        }
        index += 1;
    }
    let Some(last_portal) = portals.last() else {
        return points;
    };
    let end = last_portal[0];
    if points.last().is_none_or(|last| !near_point(*last, end)) {
        points.push(end);
    }
    points
}

pub(super) fn triangle_area_xz(a: DetourPoint, b: DetourPoint, c: DetourPoint) -> f32 {
    (b.x - a.x) * (c.z - a.z) - (c.x - a.x) * (b.z - a.z)
}
