//! Navigation route responsibilities.

#[allow(unused_imports)]
use super::cache::*;
#[allow(unused_imports)]
use super::format::*;
#[allow(unused_imports)]
use super::graph::*;
#[allow(unused_imports)]
use super::*;

pub(super) fn ensure_route_not_cancelled(cancelled: &(dyn Fn() -> bool + Sync)) -> Result<()> {
    if cancelled() {
        return Err(NavigationError::Cancelled.into());
    }
    Ok(())
}

impl DetourPoint {
    pub(super) fn to_wow_array(self) -> [f32; 3] {
        let (x, y, z) = self.to_wow();
        [x, y, z]
    }
}

pub(super) fn wow_grid(x: f32, y: f32) -> Result<(i32, i32)> {
    if !x.is_finite() || !y.is_finite() {
        bail!("navigation position is not finite");
    }
    let grid_x = (32.0 - x / WOW_GRID_SIZE) as i32;
    let grid_y = (32.0 - y / WOW_GRID_SIZE) as i32;
    if !(0..64).contains(&grid_x) || !(0..64).contains(&grid_y) {
        bail!("navigation position is outside the extracted map grid");
    }
    Ok((grid_x, grid_y))
}

pub(super) fn route_endpoint(end: (f32, f32, f32)) -> Result<DetourPoint> {
    DetourPoint::from_wow(end.0, end.1, end.2)
}

pub(super) fn validate_tile_coordinates(params: &NavMeshParams, tile: &DetourTile) -> Result<()> {
    let expected_x = ((tile.header.bounds_min[0] - params.origin[0]) / params.tile_width).round();
    let expected_y = ((tile.header.bounds_min[2] - params.origin[2]) / params.tile_height).round();
    if expected_x != tile.header.x as f32 || expected_y != tile.header.y as f32 {
        bail!("movement-map tile coordinates do not match the map parameters");
    }
    Ok(())
}

pub(super) fn route_loaded_tiles(
    tiles: &[DetourTile],
    start: DetourPoint,
    end: DetourPoint,
    allow_water: bool,
    max_visited: usize,
) -> Result<Vec<DetourPoint>> {
    let never_cancelled = || false;
    route_loaded_tiles_with_tuning(
        tiles,
        start,
        end,
        allow_water,
        max_visited,
        &NavigationTuning::default(),
        &never_cancelled,
    )
}

pub(super) fn route_loaded_tiles_with_tuning(
    tiles: &[DetourTile],
    start: DetourPoint,
    end: DetourPoint,
    allow_water: bool,
    max_visited: usize,
    tuning: &NavigationTuning,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<DetourPoint>> {
    route_loaded_tiles_with_graph(
        tiles,
        start,
        end,
        allow_water,
        max_visited,
        tuning,
        cancelled,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn route_loaded_tiles_with_graph(
    tiles: &[DetourTile],
    start: DetourPoint,
    end: DetourPoint,
    allow_water: bool,
    max_visited: usize,
    tuning: &NavigationTuning,
    cancelled: &(dyn Fn() -> bool + Sync),
    cached_graph: Option<&RouteGraph>,
) -> Result<Vec<DetourPoint>> {
    ensure_route_not_cancelled(cancelled)?;
    let requested_start = start;
    let requested_end = end;
    let start = match nearest_loaded_polygon(tiles, start, allow_water, tuning) {
        Some(start) => start,
        None => {
            let diagnostic =
                log_nearest_polygon_failure("start", tiles, start, allow_water, tuning);
            return Err(NavigationError::NoNearbyPolygon {
                endpoint: NavigationEndpoint::Start,
                detail: Some(diagnostic),
            }
            .into());
        }
    };
    let end = match nearest_loaded_polygon(tiles, end, allow_water, tuning) {
        Some(end) => end,
        None => {
            // Relative/exploration destinations are semantic requests, not authoritative
            // coordinates. A raw endpoint can land a few yards beside the walkable mesh
            // (cliff edge, building footprint, terrain seam). Keep the start strict, but
            // allow the destination to project to a nearby walkable polygon before failing.
            let projected = nearest_loaded_polygon_with_extents(
                tiles,
                end,
                tuning.destination_projection_horizontal_yards,
                // Keep destination projection on the same walkable surface.
                // Widening this value can snap a destination to another floor
                // or navigation plane and make the next movement packet look
                // like a teleport to AzerothCore. Normal terrain reconciliation
                // is the upper bound; off-mesh server transitions use their
                // explicit transition path instead.
                tuning
                    .nearest_vertical_yards
                    .min(tuning.max_terrain_height_reconcile_yards.max(1.0)),
                allow_water,
            );
            match projected {
                Some(projected) => {
                    let requested = end.to_wow();
                    let snapped = projected.1.to_wow();
                    tracing::info!(
                        requested_x = requested.0,
                        requested_y = requested.1,
                        snapped_x = snapped.0,
                        snapped_y = snapped.1,
                        snap_distance = (requested.0 - snapped.0).hypot(requested.1 - snapped.1),
                        vertical_snap = (requested.2 - snapped.2).abs(),
                        "projected movement destination onto nearby walkable navigation mesh"
                    );
                    projected
                }
                None => {
                    let diagnostic =
                        log_nearest_polygon_failure("destination", tiles, end, allow_water, tuning);
                    return Err(NavigationError::NoNearbyPolygon {
                        endpoint: NavigationEndpoint::Destination,
                        detail: Some(diagnostic),
                    }
                    .into());
                }
            }
        }
    };
    let built_graph;
    let graph = match cached_graph {
        Some(graph) => graph,
        None => {
            built_graph = build_graph(tiles, allow_water, cancelled)?;
            &built_graph
        }
    };
    match route_between_loaded_polygons(tiles, graph, start, end, max_visited, tuning, cancelled) {
        Ok(route) => Ok(route),
        Err(primary_error) => {
            // Alternate endpoint polygons are a recovery for endpoints that had
            // to be projected onto the mesh. If both requested endpoints were
            // already represented exactly, changing either polygon would route
            // to a different physical endpoint and can bridge genuinely
            // disconnected components.
            let endpoint_projection_recovery_allowed = distance3(requested_start, start.1)
                > PORTAL_EPSILON
                || distance3(requested_end, end.1) > PORTAL_EPSILON;
            if endpoint_projection_recovery_allowed
                && matches!(
                    primary_error.downcast_ref::<NavigationError>(),
                    Some(NavigationError::Disconnected { .. })
                )
            {
                let starts = nearest_loaded_polygons(
                    tiles,
                    start.1,
                    tuning.nearest_horizontal_yards,
                    tuning.nearest_vertical_yards,
                    allow_water,
                    8,
                );
                let ends = nearest_loaded_polygons(
                    tiles,
                    end.1,
                    tuning.nearest_horizontal_yards,
                    tuning.nearest_vertical_yards,
                    allow_water,
                    8,
                );
                for candidate_start in &starts {
                    for candidate_end in &ends {
                        if candidate_start.0 == start.0 && candidate_end.0 == end.0 {
                            continue;
                        }
                        if let Ok(route) = route_between_loaded_polygons(
                            tiles,
                            graph,
                            *candidate_start,
                            *candidate_end,
                            max_visited,
                            tuning,
                            cancelled,
                        ) {
                            tracing::info!(
                                "recovered navigation route using alternate endpoint polygons"
                            );
                            return Ok(route);
                        }
                    }
                }
            }
            if let Some(route) = route_to_off_mesh_transition_entry(
                tiles,
                graph,
                (start, end),
                allow_water,
                max_visited,
                tuning,
                cancelled,
            )? {
                return Ok(route);
            }
            Err(primary_error)
        }
    }
}

fn route_between_loaded_polygons(
    tiles: &[DetourTile],
    graph: &RouteGraph,
    start: (PolygonRef, DetourPoint),
    end: (PolygonRef, DetourPoint),
    max_visited: usize,
    tuning: &NavigationTuning,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<DetourPoint>> {
    if !graph.same_component(start.0, end.0) {
        return Err(NavigationError::Disconnected { explored: None }.into());
    }
    if let Some(route) = connected_direct_surface_route(
        tiles,
        graph.edges(),
        start,
        end,
        tuning.terrain_sample_spacing_yards,
        cancelled,
    )? {
        let direct = (end.1.x - start.1.x).hypot(end.1.z - start.1.z);
        let length = route
            .windows(2)
            .map(|pair| (pair[1].x - pair[0].x).hypot(pair[1].z - pair[0].z))
            .sum::<f32>();
        tracing::info!(
            direct_yards = direct,
            route_yards = length,
            execution_points = route.len(),
            "using connected near-direct mmap surface route"
        );
        return Ok(route);
    }
    let corridor = find_loaded_path_with_retry(
        tiles,
        graph,
        start.0,
        end.0,
        max_visited,
        tuning.long_route_max_visited_nodes,
        cancelled,
    )?;
    ensure_route_not_cancelled(cancelled)?;
    let route = corridor_surface_route(
        tiles,
        graph.edges(),
        &corridor,
        start.1,
        end.1,
        tuning.terrain_sample_spacing_yards,
    )?;
    tracing::debug!(
        corridor_polygons = corridor.len(),
        execution_points = route.len(),
        sample_spacing_yards = tuning.terrain_sample_spacing_yards,
        "built collision-constrained navigation corridor"
    );
    Ok(route)
}

/// When ordinary ground polygons are disconnected, an extracted Detour
/// off-mesh connection may describe the real transition between the two
/// surfaces (for example an area-trigger teleporter). The local movement
/// executor cannot safely synthesize the off-mesh traversal itself. Route only
/// to a connection entry whose exit is proven to reach the requested
/// destination. The server must then produce an authoritative position change;
/// if the caller is already at that entry and no transition occurred, fail
/// closed instead of looping or walking a fabricated vertical edge.
fn route_to_off_mesh_transition_entry(
    tiles: &[DetourTile],
    graph: &RouteGraph,
    endpoints: ((PolygonRef, DetourPoint), (PolygonRef, DetourPoint)),
    allow_water: bool,
    max_visited: usize,
    tuning: &NavigationTuning,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<DetourPoint>>> {
    const ENTRY_REACHED_YARDS: f32 = 1.0;
    let (start, end) = endpoints;
    let mut best: Option<(f32, Vec<DetourPoint>, u32, DetourPoint, DetourPoint)> = None;
    let mut start_corridors: HashMap<PolygonRef, Option<Vec<PolygonRef>>> = HashMap::new();
    let mut exits_reaching_end: HashMap<PolygonRef, bool> = HashMap::new();

    for tile in tiles {
        for connection in &tile.off_mesh_connections {
            ensure_route_not_cancelled(cancelled)?;
            let mut directions = vec![(connection.start, connection.end)];
            if connection.bidirectional {
                directions.push((connection.end, connection.start));
            }
            for (entry_point, exit_point) in directions {
                // Match normal off-mesh graph construction. The fallback may
                // snap only within the connection's own activation radius;
                // the wider generic nearest-poly tolerance would weaken the
                // transition geometry proof.
                let horizontal_extent = connection.radius.max(PORTAL_EPSILON);
                let Some(entry) = nearest_loaded_polygon_with_extents(
                    tiles,
                    entry_point,
                    horizontal_extent,
                    tuning.nearest_vertical_yards,
                    allow_water,
                ) else {
                    continue;
                };
                let Some(exit) = nearest_loaded_polygon_with_extents(
                    tiles,
                    exit_point,
                    horizontal_extent,
                    tuning.nearest_vertical_yards,
                    allow_water,
                ) else {
                    continue;
                };

                // Reject impossible ordinary halves before starting either A*.
                // Connected components are computed once with the graph and
                // make repeated disconnected off-mesh probes constant-time.
                if !graph.same_component(start.0, entry.0) || !graph.same_component(exit.0, end.0) {
                    continue;
                }

                // Both ordinary halves must be independently reachable. Cache
                // those bounded A* results because many extracted connections
                // can share the same entry/exit polygons.
                let start_corridor = start_corridors
                    .entry(entry.0)
                    .or_insert_with(|| {
                        find_loaded_path(
                            tiles,
                            graph.edges(),
                            start.0,
                            entry.0,
                            max_visited,
                            cancelled,
                        )
                        .ok()
                    })
                    .clone();
                let Some(start_corridor) = start_corridor else {
                    continue;
                };
                let exit_reaches_end = *exits_reaching_end.entry(exit.0).or_insert_with(|| {
                    find_loaded_path(tiles, graph.edges(), exit.0, end.0, max_visited, cancelled)
                        .is_ok()
                });
                if !exit_reaches_end {
                    continue;
                }

                let entry_distance = (entry.1.x - start.1.x).hypot(entry.1.z - start.1.z);
                if entry_distance <= connection.radius.max(ENTRY_REACHED_YARDS) {
                    tracing::warn!(
                        off_mesh_user_id = connection.user_id,
                        entry_distance,
                        "navigation reached a required off-mesh transition entry but the server did not move the character to the exit surface"
                    );
                    continue;
                }

                let route = match connected_direct_surface_route(
                    tiles,
                    graph.edges(),
                    start,
                    entry,
                    tuning.terrain_sample_spacing_yards,
                    cancelled,
                )? {
                    Some(route) => route,
                    None => match corridor_surface_route(
                        tiles,
                        graph.edges(),
                        &start_corridor,
                        start.1,
                        entry.1,
                        tuning.terrain_sample_spacing_yards,
                    ) {
                        Ok(route) => route,
                        Err(_) => continue,
                    },
                };
                let route_length = route
                    .windows(2)
                    .map(|pair| (pair[1].x - pair[0].x).hypot(pair[1].z - pair[0].z))
                    .sum::<f32>();
                if best
                    .as_ref()
                    .is_none_or(|(best_length, _, _, _, _)| route_length < *best_length)
                {
                    best = Some((
                        route_length,
                        route,
                        connection.user_id,
                        entry_point,
                        exit_point,
                    ));
                }
            }
        }
    }

    let Some((route_length, route, user_id, entry, exit)) = best else {
        return Ok(None);
    };
    let entry_wow = entry.to_wow();
    let exit_wow = exit.to_wow();
    tracing::info!(
        off_mesh_user_id = user_id,
        route_yards = route_length,
        entry_x = entry_wow.0,
        entry_y = entry_wow.1,
        entry_z = entry_wow.2,
        exit_x = exit_wow.0,
        exit_y = exit_wow.1,
        exit_z = exit_wow.2,
        "ground route is disconnected; routing only to a proven off-mesh transition entry and waiting for server-owned traversal"
    );
    Ok(Some(route))
}

/// Try the literal start-to-end line before invoking A*.  A* operates on
/// polygon-center costs, which can choose a needlessly long corridor even when
/// the requested line crosses a simple chain of adjacent walkable polygons.
///
/// This is not an unvalidated XYZ shortcut: every half-yard sample must remain
/// on the current polygon or one of its graph-connected neighbors, and large
/// vertical layer changes are rejected.  If any sample cannot be proven to be
/// on that connected walkable surface, normal A* corridor routing takes over.
pub(super) fn connected_direct_surface_route(
    tiles: &[DetourTile],
    graph: &HashMap<PolygonRef, Vec<GraphEdge>>,
    start: (PolygonRef, DetourPoint),
    end: (PolygonRef, DetourPoint),
    sample_spacing: f32,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<DetourPoint>>> {
    const MAX_DIRECT_SAMPLE_YARDS: f32 = 0.25;
    const HORIZONTAL_EPSILON: f32 = 0.08;
    const MAX_SAMPLE_VERTICAL_DELTA_YARDS: f32 = 1.5;

    let distance = (end.1.x - start.1.x).hypot(end.1.z - start.1.z);
    if distance <= 0.01 {
        return Ok(
            (start.0 == end.0 && (start.1.y - end.1.y).abs() <= 0.01).then(|| vec![start.1, end.1])
        );
    }
    let spacing = sample_spacing.clamp(0.25, MAX_DIRECT_SAMPLE_YARDS);
    let steps = (distance / spacing).ceil().max(1.0) as usize;
    let mut route = Vec::with_capacity(steps + 1);
    route.push(start.1);
    let mut polygon = start.0;

    for step in 1..=steps {
        ensure_route_not_cancelled(cancelled)?;
        let ratio = step as f32 / steps as f32;
        let raw = DetourPoint {
            x: start.1.x + (end.1.x - start.1.x) * ratio,
            y: start.1.y + (end.1.y - start.1.y) * ratio,
            z: start.1.z + (end.1.z - start.1.z) * ratio,
        };
        let Some(previous) = route.last().copied() else {
            return Ok(None);
        };

        let mut candidates =
            Vec::with_capacity(1 + graph.get(&polygon).map_or(0, |edges| edges.len()));
        candidates.push(polygon);
        for edge in graph.get(&polygon).into_iter().flatten() {
            if !candidates.contains(&edge.to) {
                candidates.push(edge.to);
            }
        }

        let mut best: Option<(f32, f32, PolygonRef, DetourPoint)> = None;
        for candidate_polygon in candidates {
            if candidate_polygon != polygon
                && !graph.get(&polygon).into_iter().flatten().any(|edge| {
                    edge.to == candidate_polygon && crosses_portal(previous, raw, edge.portal)
                })
            {
                continue;
            }
            let Some(tile) = tiles.get(candidate_polygon.tile) else {
                continue;
            };
            let Some(surface) = tile.closest_point_on_polygon(candidate_polygon.polygon, raw)
            else {
                continue;
            };
            let horizontal = (surface.x - raw.x).hypot(surface.z - raw.z);
            if horizontal > HORIZONTAL_EPSILON {
                continue;
            }
            let vertical_step = (surface.y - previous.y).abs();
            if vertical_step > MAX_SAMPLE_VERTICAL_DELTA_YARDS {
                continue;
            }
            let expected_vertical = (surface.y - raw.y).abs();
            let score = horizontal * 10.0 + expected_vertical + vertical_step * 0.1;
            if best
                .as_ref()
                .is_none_or(|(best_score, _, _, _)| score < *best_score)
            {
                best = Some((score, horizontal, candidate_polygon, surface));
            }
        }

        let Some((_, _, next_polygon, surface)) = best else {
            return Ok(None);
        };
        polygon = next_polygon;
        if route.last().is_none_or(|last| {
            (last.x - surface.x).hypot(last.z - surface.z) > 0.01
                || (last.y - surface.y).abs() > 0.01
        }) {
            route.push(surface);
        }
    }

    let Some(last) = route.last().copied() else {
        return Ok(None);
    };
    if (last.x - end.1.x).hypot(last.z - end.1.z) > HORIZONTAL_EPSILON
        || (last.y - end.1.y).abs() > MAX_SAMPLE_VERTICAL_DELTA_YARDS
    {
        return Ok(None);
    }
    if route.len() < 2 {
        route.push(end.1);
    } else if let Some(last) = route.last_mut() {
        *last = end.1;
    }
    Ok(Some(route))
}

/// Build an executable surface route through the Detour polygon corridor.
///
/// A* returns a polygon corridor, not a player movement spline. Walking through
/// every portal midpoint is collision-safe but creates a saw-tooth path through
/// triangular navmesh polygons, which makes the character weave, circle, and
/// sometimes briefly move away from its actual destination.
///
/// Use the standard funnel/string-pull over the exact corridor portals to pick
/// only the necessary corners, then densely validate and project every segment
/// back onto the polygons in the original corridor. If a shortcut cannot be
/// validated, fall back to the slower portal-midpoint corridor rather than
/// executing an ungrounded chord or discarding an otherwise valid A* path.
pub(super) fn corridor_surface_route(
    tiles: &[DetourTile],
    graph: &HashMap<PolygonRef, Vec<GraphEdge>>,
    corridor: &[PolygonRef],
    start: DetourPoint,
    end: DetourPoint,
    sample_spacing: f32,
) -> Result<Vec<DetourPoint>> {
    match funnel_corridor_surface_route(tiles, graph, corridor, start, end, sample_spacing, true) {
        Ok(route) => Ok(route),
        Err(rounded_error) => {
            tracing::debug!(
                %rounded_error,
                corridor_polygons = corridor.len(),
                "rounded funnel route did not fit the corridor; retrying exact Detour corners"
            );
            match funnel_corridor_surface_route(
                tiles,
                graph,
                corridor,
                start,
                end,
                sample_spacing,
                false,
            ) {
                Ok(route) => Ok(route),
                Err(error) => {
                    let fallback = corridor_midpoint_surface_route(
                        tiles,
                        graph,
                        corridor,
                        start,
                        end,
                        sample_spacing,
                    );
                    if fallback.is_ok() {
                        tracing::debug!(
                            %error,
                            corridor_polygons = corridor.len(),
                            "funnel route validation failed; using collision-safe polygon corridor"
                        );
                    } else {
                        tracing::warn!(
                            %error,
                            corridor_polygons = corridor.len(),
                            "funnel route validation and collision-safe fallback both failed"
                        );
                    }
                    fallback
                }
            }
        }
    }
}

pub(super) fn funnel_corridor_surface_route(
    tiles: &[DetourTile],
    graph: &HashMap<PolygonRef, Vec<GraphEdge>>,
    corridor: &[PolygonRef],
    start: DetourPoint,
    end: DetourPoint,
    sample_spacing: f32,
    round_corners: bool,
) -> Result<Vec<DetourPoint>> {
    let first = *corridor.first().context("navigation corridor is empty")?;
    let last = *corridor.last().context("navigation corridor is empty")?;
    let start = surface_point_in_polygon(tiles, first, start)?;
    let end = surface_point_in_polygon(tiles, last, end)?;

    let mut portals = Vec::with_capacity(corridor.len() + 1);
    portals.push([start, start]);
    for pair in corridor.windows(2) {
        let from = pair[0];
        let to = pair[1];
        let edge = graph
            .get(&from)
            .and_then(|edges| edges.iter().find(|edge| edge.to == to))
            .context("navigation corridor portal is missing")?;
        portals.push(oriented_portal(tiles, from, to, edge.portal)?);
    }
    portals.push([end, end]);

    let corners = string_pull(&portals);
    if corners.len() < 2 {
        return Err(NavigationError::NoRoute {
            detail: "navigation corridor funnel produced no traversable segment".into(),
        }
        .into());
    }

    // Detour's funnel is intentionally exact: when the corridor pinches, a
    // straight path can touch a portal endpoint. That is mathematically valid,
    // but a player following the result looks robotic because the path makes a
    // hard pivot at one navmesh vertex. Cut those pivots into short, bounded
    // quadratic arcs. Every resulting chord is still projected and validated
    // against the original A* corridor below, so this cannot create an
    // ungrounded shortcut through geometry.
    let steering_points = if round_corners {
        rounded_funnel_points(&corners)
    } else {
        corners.clone()
    };

    // Half-yard validation keeps every line segment close to the eroded Detour
    // walkable surface while still producing a small, cheap execution deque.
    const MAX_EXECUTION_SAMPLE_YARDS: f32 = 0.5;
    let spacing = sample_spacing.clamp(0.25, MAX_EXECUTION_SAMPLE_YARDS);

    let mut route = Vec::new();
    let first_surface = corridor_surface_at_point(tiles, corridor, steering_points[0])
        .context("navigation corridor start is outside its polygon corridor")?;
    route.push(first_surface);

    for pair in steering_points.windows(2) {
        append_corridor_surface_segment(tiles, corridor, pair[0], pair[1], spacing, &mut route)?;
    }
    validate_corridor_order(tiles, graph, corridor, &route)?;

    if route.len() < 2 {
        return Err(NavigationError::NoRoute {
            detail: "navigation corridor produced an empty execution route".into(),
        }
        .into());
    }

    let direct = (end.x - start.x).hypot(end.z - start.z);
    let length = route
        .windows(2)
        .map(|pair| (pair[1].x - pair[0].x).hypot(pair[1].z - pair[0].z))
        .sum::<f32>();
    tracing::debug!(
        corridor_polygons = corridor.len(),
        funnel_corners = corners.len(),
        steering_points = steering_points.len(),
        rounded_corners = round_corners,
        execution_points = route.len(),
        direct_yards = direct,
        route_yards = length,
        "built funnel-smoothed collision-constrained navigation corridor"
    );

    Ok(route)
}

/// Convert exact funnel corners into a short corner-cutting polyline.
///
/// The control point remains the original Detour corner, but the emitted path
/// does not normally touch it. A quadratic Bezier approaches and leaves the
/// corner over a radius scaled to the adjacent segment lengths. The caller
/// still validates every chord against the original polygon corridor.
pub(super) fn rounded_funnel_points(corners: &[DetourPoint]) -> Vec<DetourPoint> {
    const MAX_CORNER_RADIUS_YARDS: f32 = 1.35;
    const MIN_CORNER_RADIUS_YARDS: f32 = 0.30;
    const MAX_RADIUS_SEGMENT_FRACTION: f32 = 0.30;
    const CURVE_SAMPLE_YARDS: f32 = 0.35;
    const MIN_TURN_RADIANS: f32 = 8.0 * std::f32::consts::PI / 180.0;

    if corners.len() <= 2 {
        return corners.to_vec();
    }

    let mut points = Vec::with_capacity(corners.len() * 3);
    points.push(corners[0]);

    for index in 1..corners.len() - 1 {
        let previous = corners[index - 1];
        let corner = corners[index];
        let next = corners[index + 1];
        let incoming_x = corner.x - previous.x;
        let incoming_z = corner.z - previous.z;
        let outgoing_x = next.x - corner.x;
        let outgoing_z = next.z - corner.z;
        let incoming_len = incoming_x.hypot(incoming_z);
        let outgoing_len = outgoing_x.hypot(outgoing_z);
        if incoming_len <= 0.01 || outgoing_len <= 0.01 {
            push_distinct_point(&mut points, corner);
            continue;
        }

        let in_x = incoming_x / incoming_len;
        let in_z = incoming_z / incoming_len;
        let out_x = outgoing_x / outgoing_len;
        let out_z = outgoing_z / outgoing_len;
        let dot = (in_x * out_x + in_z * out_z).clamp(-1.0, 1.0);
        let turn = dot.acos();
        if turn < MIN_TURN_RADIANS {
            push_distinct_point(&mut points, corner);
            continue;
        }

        let radius = MAX_CORNER_RADIUS_YARDS
            .min(incoming_len * MAX_RADIUS_SEGMENT_FRACTION)
            .min(outgoing_len * MAX_RADIUS_SEGMENT_FRACTION);
        if radius < MIN_CORNER_RADIUS_YARDS {
            push_distinct_point(&mut points, corner);
            continue;
        }

        let entry = DetourPoint {
            x: corner.x - in_x * radius,
            y: corner.y + (previous.y - corner.y) * (radius / incoming_len),
            z: corner.z - in_z * radius,
        };
        let exit = DetourPoint {
            x: corner.x + out_x * radius,
            y: corner.y + (next.y - corner.y) * (radius / outgoing_len),
            z: corner.z + out_z * radius,
        };
        push_distinct_point(&mut points, entry);

        let approximate_curve_length = (entry.x - corner.x).hypot(entry.z - corner.z)
            + (exit.x - corner.x).hypot(exit.z - corner.z);
        let samples = (approximate_curve_length / CURVE_SAMPLE_YARDS)
            .ceil()
            .max(2.0) as usize;
        for sample in 1..samples {
            let t = sample as f32 / samples as f32;
            let one_minus_t = 1.0 - t;
            let point = DetourPoint {
                x: one_minus_t * one_minus_t * entry.x
                    + 2.0 * one_minus_t * t * corner.x
                    + t * t * exit.x,
                y: one_minus_t * one_minus_t * entry.y
                    + 2.0 * one_minus_t * t * corner.y
                    + t * t * exit.y,
                z: one_minus_t * one_minus_t * entry.z
                    + 2.0 * one_minus_t * t * corner.z
                    + t * t * exit.z,
            };
            push_distinct_point(&mut points, point);
        }
        push_distinct_point(&mut points, exit);
    }

    push_distinct_point(&mut points, *corners.last().expect("non-empty corners"));
    points
}

pub(super) fn push_distinct_point(points: &mut Vec<DetourPoint>, point: DetourPoint) {
    if points.last().is_none_or(|last| !near_point(*last, point)) {
        points.push(point);
    }
}

pub(super) fn corridor_midpoint_surface_route(
    tiles: &[DetourTile],
    graph: &HashMap<PolygonRef, Vec<GraphEdge>>,
    corridor: &[PolygonRef],
    start: DetourPoint,
    end: DetourPoint,
    sample_spacing: f32,
) -> Result<Vec<DetourPoint>> {
    let first = *corridor.first().context("navigation corridor is empty")?;
    let mut route = Vec::new();
    let mut current = surface_point_in_polygon(tiles, first, start)?;
    route.push(current);
    let mut steering_corners = vec![current];

    for (index, polygon) in corridor.iter().copied().enumerate() {
        current = surface_point_in_polygon(tiles, polygon, current)?;
        if let Some(last) = route.last_mut()
            && (last.x - current.x).abs() <= PORTAL_EPSILON
            && (last.z - current.z).abs() <= PORTAL_EPSILON
            && (last.y - current.y).abs()
                <= tiles[polygon.tile].header.walkable_climb + PORTAL_EPSILON
        {
            last.y = current.y;
        }

        let target = if let Some(next) = corridor.get(index + 1).copied() {
            let edge = graph
                .get(&polygon)
                .and_then(|edges| edges.iter().find(|edge| edge.to == next))
                .context("navigation corridor portal is missing")?;
            let midpoint = DetourPoint {
                x: (edge.portal[0].x + edge.portal[1].x) * 0.5,
                y: (edge.portal[0].y + edge.portal[1].y) * 0.5,
                z: (edge.portal[0].z + edge.portal[1].z) * 0.5,
            };
            surface_point_in_polygon(tiles, polygon, midpoint)?
        } else {
            surface_point_in_polygon(tiles, polygon, end)?
        };

        append_polygon_surface_segment(
            tiles,
            polygon,
            current,
            target,
            sample_spacing,
            &mut route,
        )?;
        current = target;
        push_distinct_point(&mut steering_corners, target);
    }

    if route.len() < 2 {
        route.push(surface_point_in_polygon(tiles, first, end)?);
    }

    // The portal-midpoint route is our conservative fallback when the standard
    // funnel cannot fit the corridor. It is safe but can look mechanically
    // angular. Round those interior midpoint corners, then accept the smoother
    // version only if every dense chord still resolves to the original A*
    // corridor. Any questionable corner keeps the exact conservative route.
    let rounded = rounded_funnel_points(&steering_corners);
    if rounded.len() >= 2 {
        let spacing = sample_spacing.clamp(0.25, 0.5);
        let mut smoothed = Vec::new();
        if let Some(first_point) = rounded.first().copied()
            && let Some(first_surface) = corridor_surface_at_point(tiles, corridor, first_point)
        {
            smoothed.push(first_surface);
            let mut valid = true;
            for pair in rounded.windows(2) {
                if let Err(error) = append_corridor_surface_segment(
                    tiles,
                    corridor,
                    pair[0],
                    pair[1],
                    spacing,
                    &mut smoothed,
                ) {
                    tracing::debug!(
                        %error,
                        corridor_polygons = corridor.len(),
                        "rounded midpoint fallback left corridor; keeping conservative route"
                    );
                    valid = false;
                    break;
                }
            }
            if valid
                && smoothed.len() >= 2
                && validate_corridor_order(tiles, graph, corridor, &smoothed).is_ok()
            {
                tracing::info!(
                    corridor_polygons = corridor.len(),
                    steering_corners = steering_corners.len(),
                    execution_points = smoothed.len(),
                    "using rounded collision-safe polygon corridor fallback"
                );
                return Ok(smoothed);
            }
        }
    }

    tracing::info!(
        corridor_polygons = corridor.len(),
        execution_points = route.len(),
        "using collision-safe unsmoothed corridor fallback"
    );
    Ok(route)
}

/// A shortcut must cross actual portals, not merely land on another floor.
fn crosses_portal(start: DetourPoint, end: DetourPoint, portal: [DetourPoint; 2]) -> bool {
    let cross = |ax: f32, az: f32, bx: f32, bz: f32| ax * bz - az * bx;
    let dx = end.x - start.x;
    let dz = end.z - start.z;
    let px = portal[1].x - portal[0].x;
    let pz = portal[1].z - portal[0].z;
    let qx = portal[0].x - start.x;
    let qz = portal[0].z - start.z;
    let denominator = cross(dx, dz, px, pz);
    if denominator.abs() <= 1e-6 {
        // Collinear movement can start on a shared edge.
        let length = px * px + pz * pz;
        if length <= 1e-6 {
            return false;
        }
        let t = ((start.x - portal[0].x) * px + (start.z - portal[0].z) * pz) / length;
        return (0.0..=1.0).contains(&t) && cross(qx, qz, px, pz).abs() / length.sqrt() <= 0.001;
    }
    let t = cross(qx, qz, px, pz) / denominator;
    let u = cross(qx, qz, dx, dz) / denominator;
    (-0.0001..=1.0001).contains(&t) && (-0.0001..=1.0001).contains(&u)
}

/// Retain the ordered polygon corridor while checking the sampled polyline.
/// Uncertain smoothing falls back to explicit per-polygon portal traversal.
fn validate_corridor_order(
    tiles: &[DetourTile],
    graph: &HashMap<PolygonRef, Vec<GraphEdge>>,
    corridor: &[PolygonRef],
    route: &[DetourPoint],
) -> Result<()> {
    let mut cursor = 0;
    let supports = |polygon: PolygonRef, point: DetourPoint| {
        tiles[polygon.tile]
            .closest_point_on_polygon(polygon.polygon, point)
            .is_some_and(|surface| {
                (surface.x - point.x).hypot(surface.z - point.z) <= 0.001
                    && (surface.y - point.y).abs() <= 0.05
            })
    };
    let first = *corridor.first().context("empty polygon corridor")?;
    let start = *route.first().context("empty surface route")?;
    if !supports(first, start) {
        bail!("route starts on a different corridor surface");
    }
    for segment in route.windows(2) {
        while !supports(corridor[cursor], segment[1]) {
            let next = *corridor
                .get(cursor + 1)
                .context("route left its ordered corridor")?;
            let edge = graph
                .get(&corridor[cursor])
                .into_iter()
                .flatten()
                .find(|edge| edge.to == next)
                .context("missing ordered corridor portal")?;
            if !crosses_portal(segment[0], segment[1], edge.portal) {
                bail!("route changes polygon without crossing its portal");
            }
            cursor += 1;
        }
    }
    // A destination on a shared boundary can still belong to the last polygon.
    if cursor + 1 < corridor.len() && !supports(*corridor.last().unwrap(), *route.last().unwrap()) {
        bail!("route did not reach its destination surface");
    }
    Ok(())
}

pub(super) fn append_polygon_surface_segment(
    tiles: &[DetourTile],
    polygon: PolygonRef,
    start: DetourPoint,
    end: DetourPoint,
    sample_spacing: f32,
    route: &mut Vec<DetourPoint>,
) -> Result<()> {
    let tile = tiles
        .get(polygon.tile)
        .context("navigation corridor tile is unavailable")?;
    let distance = (end.x - start.x).hypot(end.z - start.z);
    let steps = (distance / sample_spacing.max(0.25)).ceil().max(1.0) as usize;
    for step in 1..=steps {
        let ratio = step as f32 / steps as f32;
        let x = start.x + (end.x - start.x) * ratio;
        let z = start.z + (end.z - start.z) * ratio;
        let y = tile
            .surface_height(polygon.polygon, x, z)
            .context("navigation corridor sample has no walkable surface")?;
        let surface = DetourPoint { x, y, z };
        if route.last().is_none_or(|last| {
            (last.x - surface.x).hypot(last.z - surface.z) > 0.01
                || (last.y - surface.y).abs() > 0.01
        }) {
            route.push(surface);
        }
    }
    Ok(())
}

pub(super) fn surface_point_in_polygon(
    tiles: &[DetourTile],
    polygon: PolygonRef,
    point: DetourPoint,
) -> Result<DetourPoint> {
    let tile = tiles
        .get(polygon.tile)
        .context("navigation corridor tile is unavailable")?;
    let projected = tile
        .closest_point_on_polygon(polygon.polygon, point)
        .context("navigation corridor point has no polygon surface")?;
    Ok(projected)
}

/// Resolve an X/Z sample to the closest surface that belongs to the original
/// A* corridor. This is the safety check around funnel smoothing: the model and
/// executor never get to trust a straight chord unless each dense sample stays
/// on one of the corridor's walkable polygons.
pub(super) fn corridor_surface_at_point(
    tiles: &[DetourTile],
    corridor: &[PolygonRef],
    point: DetourPoint,
) -> Option<DetourPoint> {
    const HORIZONTAL_EPSILON: f32 = 0.08;
    corridor
        .iter()
        .copied()
        .filter_map(|polygon| {
            let tile = tiles.get(polygon.tile)?;
            let candidate = tile.closest_point_on_polygon(polygon.polygon, point)?;
            let horizontal = (candidate.x - point.x).hypot(candidate.z - point.z);
            if horizontal > HORIZONTAL_EPSILON {
                return None;
            }
            // Overlapping floors can share X/Z. Keep the surface closest to the
            // expected funnel height so a ground route cannot jump layers.
            let vertical = (candidate.y - point.y).abs();
            Some((horizontal, vertical, candidate))
        })
        .min_by(|left, right| {
            left.0
                .total_cmp(&right.0)
                .then_with(|| left.1.total_cmp(&right.1))
        })
        .map(|(_, _, candidate)| candidate)
}

pub(super) fn append_corridor_surface_segment(
    tiles: &[DetourTile],
    corridor: &[PolygonRef],
    start: DetourPoint,
    end: DetourPoint,
    sample_spacing: f32,
    route: &mut Vec<DetourPoint>,
) -> Result<()> {
    let distance = (end.x - start.x).hypot(end.z - start.z);
    let steps = (distance / sample_spacing.max(0.25)).ceil().max(1.0) as usize;
    for step in 1..=steps {
        let ratio = step as f32 / steps as f32;
        let raw = DetourPoint {
            x: start.x + (end.x - start.x) * ratio,
            y: start.y + (end.y - start.y) * ratio,
            z: start.z + (end.z - start.z) * ratio,
        };
        let surface = corridor_surface_at_point(tiles, corridor, raw)
            .context("funnel shortcut leaves the walkable navigation corridor")?;
        if route.last().is_none_or(|last| {
            (last.x - surface.x).hypot(last.z - surface.z) > 0.01
                || (last.y - surface.y).abs() > 0.01
        }) {
            route.push(surface);
        }
    }
    Ok(())
}

pub(super) fn log_nearest_polygon_failure(
    endpoint: &str,
    tiles: &[DetourTile],
    point: DetourPoint,
    allow_water: bool,
    tuning: &NavigationTuning,
) -> String {
    let wow = point.to_wow();
    let mut total_polygons = 0usize;
    let mut walkable_polygons = 0usize;
    let mut closest: Option<(f32, f32, f32, usize, usize, DetourPoint)> = None;
    for (tile_index, tile) in tiles.iter().enumerate() {
        total_polygons += tile.polygons.len();
        for (polygon_index, polygon) in tile.polygons.iter().enumerate() {
            if !polygon.walkable(allow_water) {
                continue;
            }
            walkable_polygons += 1;
            let Some(candidate) = tile.closest_point_on_polygon(polygon_index, point) else {
                continue;
            };
            let horizontal =
                ((candidate.x - point.x).powi(2) + (candidate.z - point.z).powi(2)).sqrt();
            let vertical = (candidate.y - point.y).abs();
            let distance = distance3(candidate, point);
            if closest.as_ref().is_none_or(|current| distance < current.0) {
                closest = Some((
                    distance,
                    horizontal,
                    vertical,
                    tile_index,
                    polygon_index,
                    candidate,
                ));
            }
        }
    }
    if let Some((distance, horizontal, vertical, tile_index, polygon_index, candidate)) = closest {
        let candidate_wow = candidate.to_wow();
        let tile = &tiles[tile_index];
        tracing::debug!(
            endpoint,
            wow_x = wow.0,
            wow_y = wow.1,
            wow_z = wow.2,
            detour_x = point.x,
            detour_y = point.y,
            detour_z = point.z,
            loaded_tiles = tiles.len(),
            total_polygons,
            walkable_polygons,
            closest_tile_index = tile_index,
            closest_tile_x = tile.header.x,
            closest_tile_y = tile.header.y,
            closest_tile_layer = tile.header.layer,
            closest_polygon = polygon_index,
            closest_distance = distance,
            horizontal_offset = horizontal,
            vertical_offset = vertical,
            closest_wow_x = candidate_wow.0,
            closest_wow_y = candidate_wow.1,
            closest_wow_z = candidate_wow.2,
            allowed_horizontal = tuning.nearest_horizontal_yards,
            allowed_vertical = tuning.nearest_vertical_yards,
            allow_water,
            "nearest walkable polygon diagnostic"
        );
        format!(
            "endpoint={endpoint} point=({:.3},{:.3},{:.3}) loaded_tiles={} total_polygons={} walkable_polygons={} closest_tile=({}, {}, layer {}) closest_polygon={} closest=({:.3},{:.3},{:.3}) horizontal_offset={:.3} vertical_offset={:.3} distance={:.3} allowed_horizontal={:.3} allowed_vertical={:.3} allow_water={}",
            wow.0,
            wow.1,
            wow.2,
            tiles.len(),
            total_polygons,
            walkable_polygons,
            tile.header.x,
            tile.header.y,
            tile.header.layer,
            polygon_index,
            candidate_wow.0,
            candidate_wow.1,
            candidate_wow.2,
            horizontal,
            vertical,
            distance,
            tuning.nearest_horizontal_yards,
            tuning.nearest_vertical_yards,
            allow_water
        )
    } else {
        tracing::debug!(
            endpoint,
            wow_x = wow.0,
            wow_y = wow.1,
            wow_z = wow.2,
            loaded_tiles = tiles.len(),
            total_polygons,
            walkable_polygons,
            allow_water,
            "nearest walkable polygon diagnostic found no walkable polygon candidates"
        );
        format!(
            "endpoint={endpoint} point=({:.3},{:.3},{:.3}) loaded_tiles={} total_polygons={} walkable_polygons={} no_walkable_candidate=true allow_water={}",
            wow.0,
            wow.1,
            wow.2,
            tiles.len(),
            total_polygons,
            walkable_polygons,
            allow_water
        )
    }
}
