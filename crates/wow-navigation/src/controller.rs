use crate::{
    NavigationError, TerrainSampler,
    detour::{NavigationData, RouteSurface},
};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use wow_domain::{Vec3, WorldPosition};

const MOVEMENTFLAG_DISABLE_GRAVITY: u32 = 0x0000_0400;
const MOVEMENTFLAG_CAN_FLY: u32 = 0x0100_0000;
const MOVEMENTFLAG_FLYING: u32 = 0x0200_0000;
const MAX_GROUND_VERTICAL_STEP: f32 = 3.5;
const ROUTE_DESTINATION_EPSILON: f32 = 0.75;
const INTERMEDIATE_WAYPOINT_RANGE: f32 = 0.65;
const ROUTE_LOOKAHEAD_DISTANCE: f32 = 4.5;
const ROUTE_LOOKAHEAD_MAX_ANGLE: f32 = 0.261_799_4; // 15 degrees
const LOCAL_VISION_MAX_AGE: std::time::Duration = std::time::Duration::from_millis(250);
const LOCAL_VISION_MAX_DROP_YARDS: f32 = 2.5;
pub const MAX_CONCURRENT_ROUTE_PLANS: usize = 1;
pub const ROUTE_PLANNING_DEADLINE: Duration = Duration::from_secs(12);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocomotionMode {
    Ground,
    Flight,
}

/// Source that is allowed to determine the execution step Z coordinate.
///
/// Terrain is suitable for unrouted outdoor grounding. Once Detour has
/// authorized a ground corridor, the navmesh surface owns vertical topology so
/// stacked floors, caves, bridges, and interiors cannot be flattened onto the
/// raw map height field. Flight preserves explicit 3-D movement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerticalAuthority {
    Terrain,
    NavMesh,
    ThreeD,
}

impl LocomotionMode {
    pub fn from_server_flags(controlled_mover: bool, flags: u32) -> Self {
        if controlled_mover
            && flags & (MOVEMENTFLAG_DISABLE_GRAVITY | MOVEMENTFLAG_CAN_FLY | MOVEMENTFLAG_FLYING)
                != 0
        {
            Self::Flight
        } else {
            Self::Ground
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovementStep {
    pub next: Vec3,
    pub remaining: f32,
}

#[derive(Clone, Debug)]
struct RouteCursor {
    map: u32,
    destination: Vec3,
    points: Vec<Vec3>,
    surfaces: Vec<Option<RouteSurface>>,
    index: usize,
}

/// A route computed off the lane thread and ready for installation.
#[derive(Clone, Debug)]
pub struct PlannedRoute {
    map: u32,
    destination: Vec3,
    points: Vec<Vec3>,
    surfaces: Vec<Option<RouteSurface>>,
}

impl PlannedRoute {
    pub fn points(&self) -> &[Vec3] {
        &self.points
    }
}

#[derive(Clone)]
pub struct MovementController {
    terrain: TerrainSampler,
    maximum_step: f32,
    navigation: Option<Arc<NavigationData>>,
    route: Arc<Mutex<Option<RouteCursor>>>,
    route_planning_slots: Arc<Semaphore>,
}

impl MovementController {
    pub fn new(terrain: TerrainSampler) -> Self {
        Self::with_navigation(terrain, None)
    }

    pub fn new_with_mmaps(
        terrain: TerrainSampler,
        mmaps_dir: impl AsRef<Path>,
    ) -> Result<Self, NavigationError> {
        let navigation = NavigationData::new(mmaps_dir.as_ref())
            .map_err(|_| NavigationError::MissingNavigationData)?;
        Ok(Self::with_navigation(terrain, Some(Arc::new(navigation))))
    }

    fn with_navigation(terrain: TerrainSampler, navigation: Option<Arc<NavigationData>>) -> Self {
        Self {
            terrain,
            maximum_step: 1.5,
            navigation,
            route: Arc::new(Mutex::new(None)),
            route_planning_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_ROUTE_PLANS)),
        }
    }

    pub fn with_maximum_step(mut self, value: f32) -> Self {
        if value.is_finite() && value > 0.0 {
            self.maximum_step = value;
        }
        self
    }

    pub fn clear_route(&self) {
        *lock_recover(&self.route) = None;
    }

    pub fn route_is_ready(&self, map: u32, destination: Vec3) -> bool {
        lock_recover(&self.route).as_ref().is_some_and(|route| {
            route.map == map
                && route.destination.distance(destination) <= ROUTE_DESTINATION_EPSILON
                && route.index < route.points.len()
        })
    }

    pub fn has_navigation_data(&self) -> bool {
        self.navigation.is_some()
    }

    /// Accept an authored waypoint only when MMAP projects it onto a nearby
    /// ground polygon without materially changing its position.
    pub fn validate_grounded_waypoint(
        &self,
        position: WorldPosition,
    ) -> Result<(), NavigationError> {
        let navigation = self
            .navigation
            .as_ref()
            .ok_or(NavigationError::MissingNavigationData)?;
        if !position.point.is_finite() || !position.orientation.is_finite() {
            return Err(NavigationError::NoRoute);
        }
        let projected = navigation
            .project_walkable_position(
                position.map,
                (position.point.x, position.point.y, position.point.z),
                false,
                &|| false,
            )
            .map_err(|_| NavigationError::NoRoute)?;
        let horizontal = (projected[0] - position.point.x).hypot(projected[1] - position.point.y);
        let vertical = (projected[2] - position.point.z).abs();
        if horizontal > 1.0 || vertical > 1.0 {
            return Err(NavigationError::NoRoute);
        }
        let sample = [[projected[0], projected[1], projected[2]]];
        let surface = navigation
            .route_surfaces(position.map, &sample)
            .into_iter()
            .next()
            .flatten()
            .ok_or(NavigationError::NoRoute)?;
        if surface.kind != crate::detour::SurfaceKind::Ground {
            return Err(NavigationError::NoRoute);
        }
        Ok(())
    }

    /// Project a map-aware goal's horizontal coordinates onto ground. Require
    /// a close horizontal match; the source may not know the destination Z.
    pub fn project_grounded_waypoint(
        &self,
        position: WorldPosition,
    ) -> Result<WorldPosition, NavigationError> {
        let navigation = self
            .navigation
            .as_ref()
            .ok_or(NavigationError::MissingNavigationData)?;
        if !position.point.is_finite() || !position.orientation.is_finite() {
            return Err(NavigationError::NoRoute);
        }
        let projected = navigation
            .project_walkable_position(
                position.map,
                (position.point.x, position.point.y, position.point.z),
                false,
                &|| false,
            )
            .map_err(|_| NavigationError::NoRoute)?;
        if (projected[0] - position.point.x).hypot(projected[1] - position.point.y) > 1.0 {
            return Err(NavigationError::NoRoute);
        }
        let sample = [[projected[0], projected[1], projected[2]]];
        let surface = navigation
            .route_surfaces(position.map, &sample)
            .into_iter()
            .next()
            .flatten()
            .ok_or(NavigationError::NoRoute)?;
        if surface.kind != crate::detour::SurfaceKind::Ground {
            return Err(NavigationError::NoRoute);
        }
        Ok(WorldPosition {
            map: position.map,
            point: Vec3::new(projected[0], projected[1], projected[2]),
            orientation: position.orientation,
        })
    }

    /// Classify the server-observed position against a nearby Detour floor.
    /// Missing or distant mesh data stays unknown.
    pub fn surface_kind_at(&self, position: WorldPosition) -> Option<crate::detour::SurfaceKind> {
        let navigation = self.navigation.as_ref()?;
        let point = [[position.point.x, position.point.y, position.point.z]];
        navigation
            .route_surfaces(position.map, &point)
            .into_iter()
            .next()
            .flatten()
            .map(|surface| surface.kind)
    }

    pub fn plan_route_cancellable(
        &self,
        current: WorldPosition,
        destination: Vec3,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<PlannedRoute, NavigationError> {
        let started_at = Instant::now();
        let navigation = self
            .navigation
            .as_ref()
            .ok_or(NavigationError::MissingNavigationData)?;
        let route_cancelled = || cancelled() || started_at.elapsed() >= ROUTE_PLANNING_DEADLINE;
        let route_points = navigation
            .route_cancellable(
                current.map,
                (current.point.x, current.point.y, current.point.z),
                (destination.x, destination.y, destination.z),
                false,
                &route_cancelled,
            )
            .map_err(|_| route_planning_error(cancelled, started_at))?;
        let surfaces = navigation
            .route_surfaces_cancellable(current.map, &route_points, &route_cancelled)
            .map_err(|_| route_planning_error(cancelled, started_at))?;
        let points: Vec<Vec3> = route_points
            .into_iter()
            .map(|point| Vec3::new(point[0], point[1], point[2]))
            .collect();
        if points.is_empty() {
            return Err(NavigationError::NoRoute);
        }
        Ok(PlannedRoute {
            map: current.map,
            destination,
            points,
            surfaces,
        })
    }

    /// Limit route planning to one active job per shared controller.
    pub fn plan_route_bounded_cancellable(
        &self,
        current: WorldPosition,
        destination: Vec3,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<PlannedRoute, NavigationError> {
        let _permit = self.try_acquire_route_planning_slot()?;
        self.plan_route_cancellable(current, destination, cancelled)
    }

    pub fn try_acquire_route_planning_slot(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, NavigationError> {
        self.route_planning_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| NavigationError::RoutePlannerBusy)
    }

    pub fn install_route(&self, plan: PlannedRoute, current: Vec3) {
        let index = usize::from(plan.points.len() > 1);
        *lock_recover(&self.route) = Some(RouteCursor {
            map: plan.map,
            destination: plan.destination,
            points: plan.points,
            surfaces: plan.surfaces,
            index,
        });
        self.advance_route_cursor(current);
    }

    pub fn next_step(
        &self,
        current: WorldPosition,
        destination: Vec3,
        acceptable_range: f32,
        mode: LocomotionMode,
    ) -> Result<Option<MovementStep>, NavigationError> {
        if mode == LocomotionMode::Flight {
            self.clear_route();
            return next_step(
                current.point,
                destination,
                self.maximum_step,
                acceptable_range,
                mode,
            );
        }
        let Some(navigation) = self.navigation.as_ref() else {
            let Some(mut step) = next_step(
                current.point,
                destination,
                self.maximum_step,
                acceptable_range,
                LocomotionMode::Ground,
            )?
            else {
                return Ok(None);
            };
            let ground = self
                .terrain
                .ground_height(current.map, step.next.x, step.next.y)?;
            if (ground - current.point.z).abs() > MAX_GROUND_VERTICAL_STEP {
                return Err(NavigationError::FloorDiscontinuity);
            }
            step.next.z = ground;
            return Ok(Some(step));
        };
        let (waypoint, route_index) =
            self.ground_route_waypoint(navigation, current, destination)?;
        let raw = self.routed_ground_step(current, waypoint, destination, acceptable_range)?;
        let Some(mut step) = raw else {
            // We reached this corridor waypoint. Advance once and immediately
            // calculate toward the next one so the 250 ms clock does not stall.
            self.advance_route_cursor(current.point);
            let (waypoint, route_index) =
                self.ground_route_waypoint(navigation, current, destination)?;
            let Some(mut step) =
                self.routed_ground_step(current, waypoint, destination, acceptable_range)?
            else {
                return Ok(None);
            };
            self.apply_routed_ground_height(navigation, current, waypoint, route_index, &mut step)?;
            self.validate_local_geometry(navigation, current, step)?;
            return Ok(Some(step));
        };
        self.apply_routed_ground_height(navigation, current, waypoint, route_index, &mut step)?;
        self.validate_local_geometry(navigation, current, step)?;
        Ok(Some(step))
    }

    fn validate_local_geometry(
        &self,
        navigation: &NavigationData,
        current: WorldPosition,
        step: MovementStep,
    ) -> Result<(), NavigationError> {
        let middle = Vec3::new(
            (current.point.x + step.next.x) * 0.5,
            (current.point.y + step.next.y) * 0.5,
            (current.point.z + step.next.z) * 0.5,
        );
        let probes = [
            [current.point.x, current.point.y, current.point.z],
            [middle.x, middle.y, middle.z],
            [step.next.x, step.next.y, step.next.z],
        ];
        let sample = navigation
            .local_geometry_sample(current.map, &probes)
            .ok_or(NavigationError::NoRoute)?;
        match sample.assess(
            std::time::Instant::now(),
            LOCAL_VISION_MAX_AGE,
            LOCAL_VISION_MAX_DROP_YARDS,
        ) {
            crate::detour::LocalRouteAssessment::Clear => Ok(()),
            crate::detour::LocalRouteAssessment::UnsafeDrop => {
                Err(NavigationError::FloorDiscontinuity)
            }
            crate::detour::LocalRouteAssessment::Blocked
            | crate::detour::LocalRouteAssessment::Unknown
            | crate::detour::LocalRouteAssessment::Stale => Err(NavigationError::NoRoute),
        }
    }

    fn routed_ground_step(
        &self,
        current: WorldPosition,
        waypoint: Vec3,
        destination: Vec3,
        acceptable_range: f32,
    ) -> Result<Option<MovementStep>, NavigationError> {
        let final_waypoint = waypoint.distance(destination) <= ROUTE_DESTINATION_EPSILON;
        let step_range = if final_waypoint {
            acceptable_range
        } else {
            INTERMEDIATE_WAYPOINT_RANGE
        };
        next_step(
            current.point,
            waypoint,
            self.maximum_step,
            step_range,
            LocomotionMode::Ground,
        )
    }

    /// Keep execution on the Detour-authorized vertical layer. Route-surface
    /// identity is preferred; if optional surface metadata is unavailable, the
    /// already-validated route segment remains the vertical authority. Raw
    /// terrain height is diagnostic/fallback data only and never overwrites a
    /// valid routed layer.
    fn apply_routed_ground_height(
        &self,
        navigation: &NavigationData,
        current: WorldPosition,
        waypoint: Vec3,
        route_index: usize,
        step: &mut MovementStep,
    ) -> Result<(), NavigationError> {
        let expected_z = route_segment_height(current.point, waypoint, step.next);
        let surfaces = {
            let guard = lock_recover(&self.route);
            let route = guard.as_ref().ok_or(NavigationError::NoRoute)?;
            let current_surface = route.surfaces.get(route_index).copied().flatten();
            let previous_surface = route_index
                .checked_sub(1)
                .and_then(|index| route.surfaces.get(index))
                .copied()
                .flatten();
            [previous_surface, current_surface]
        };

        let probe = [step.next.x, step.next.y, expected_z];
        for surface in surfaces.into_iter().flatten() {
            if let Some(height) = navigation.height_on_route_surface(surface, probe) {
                if (height - current.point.z).abs() > MAX_GROUND_VERTICAL_STEP {
                    return Err(NavigationError::FloorDiscontinuity);
                }
                step.next.z = height;
                return Ok(());
            }
        }

        // Surface metadata is advisory and may be absent after cache eviction or
        // a soft-budget cutoff. The route itself was already projected onto the
        // navmesh, so interpolate along that accepted segment rather than
        // snapping to the raw MAPS terrain layer.
        if (expected_z - current.point.z).abs() > MAX_GROUND_VERTICAL_STEP {
            return Err(NavigationError::FloorDiscontinuity);
        }
        step.next.z = expected_z;
        Ok(())
    }

    fn ground_route_waypoint(
        &self,
        navigation: &NavigationData,
        current: WorldPosition,
        destination: Vec3,
    ) -> Result<(Vec3, usize), NavigationError> {
        let needs_route = {
            let guard = lock_recover(&self.route);
            guard.as_ref().is_none_or(|route| {
                route.map != current.map
                    || route.destination.distance(destination) > ROUTE_DESTINATION_EPSILON
                    || route.index >= route.points.len()
            })
        };
        if needs_route {
            let points = navigation
                .route(
                    current.map,
                    (current.point.x, current.point.y, current.point.z),
                    (destination.x, destination.y, destination.z),
                    false,
                )
                .map_err(|_| NavigationError::NoRoute)?;
            let route_points: Vec<[f32; 3]> = points;
            let surfaces = navigation.route_surfaces(current.map, &route_points);
            let points: Vec<Vec3> = route_points
                .into_iter()
                .map(|point| Vec3::new(point[0], point[1], point[2]))
                .collect();
            if points.is_empty() {
                return Err(NavigationError::NoRoute);
            }
            let index = usize::from(points.len() > 1);
            *lock_recover(&self.route) = Some(RouteCursor {
                map: current.map,
                destination,
                points,
                surfaces,
                index,
            });
        }
        self.advance_route_cursor(current.point);
        let guard = lock_recover(&self.route);
        let route = guard.as_ref().ok_or(NavigationError::NoRoute)?;
        Ok((
            route
                .points
                .get(route.index)
                .copied()
                .unwrap_or(route.destination),
            route.index,
        ))
    }

    fn advance_route_cursor(&self, current: Vec3) {
        let mut guard = lock_recover(&self.route);
        let Some(route) = guard.as_mut() else {
            return;
        };
        while route.index + 1 < route.points.len() {
            let point = route.points[route.index];
            if (point.x - current.x).hypot(point.y - current.y) <= INTERMEDIATE_WAYPOINT_RANGE {
                route.index += 1;
                continue;
            }
            let next = route.points[route.index + 1];
            if should_skip_dense_collinear_waypoint(current, point, next) {
                route.index += 1;
                continue;
            }
            break;
        }
    }
}

fn route_planning_error(
    cancelled: &(dyn Fn() -> bool + Sync),
    started_at: Instant,
) -> NavigationError {
    if cancelled() {
        NavigationError::RoutePlanningCancelled
    } else if started_at.elapsed() >= ROUTE_PLANNING_DEADLINE {
        NavigationError::RoutePlanningDeadlineExceeded
    } else {
        NavigationError::NoRoute
    }
}

fn route_segment_height(current: Vec3, waypoint: Vec3, step: Vec3) -> f32 {
    let total = (waypoint.x - current.x).hypot(waypoint.y - current.y);
    if !total.is_finite() || total <= 0.001 {
        return waypoint.z;
    }
    let travelled = (step.x - current.x).hypot(step.y - current.y);
    let fraction = (travelled / total).clamp(0.0, 1.0);
    current.z + (waypoint.z - current.z) * fraction
}

fn should_skip_dense_collinear_waypoint(current: Vec3, point: Vec3, next: Vec3) -> bool {
    let next_distance = (next.x - current.x).hypot(next.y - current.y);
    if next_distance > ROUTE_LOOKAHEAD_DISTANCE {
        return false;
    }
    let ax = point.x - current.x;
    let ay = point.y - current.y;
    let bx = next.x - current.x;
    let by = next.y - current.y;
    let alen = ax.hypot(ay);
    let blen = bx.hypot(by);
    if alen < 0.001 || blen < 0.001 {
        return true;
    }
    let dot = ((ax * bx + ay * by) / (alen * blen)).clamp(-1.0, 1.0);
    dot.acos() <= ROUTE_LOOKAHEAD_MAX_ANGLE
}

fn lock_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub fn next_step(
    current: Vec3,
    destination: Vec3,
    maximum_step: f32,
    acceptable_range: f32,
    mode: LocomotionMode,
) -> Result<Option<MovementStep>, NavigationError> {
    if !current.is_finite()
        || !destination.is_finite()
        || !maximum_step.is_finite()
        || !acceptable_range.is_finite()
        || maximum_step <= 0.0
        || acceptable_range < 0.0
    {
        return Err(NavigationError::InvalidCoordinate);
    }
    match mode {
        LocomotionMode::Ground => {
            let dx = destination.x - current.x;
            let dy = destination.y - current.y;
            let horizontal = dx.hypot(dy);
            if horizontal <= acceptable_range {
                return Ok(None);
            }
            let step = maximum_step.min((horizontal - acceptable_range).max(0.25));
            let scale = step / horizontal.max(0.001);
            Ok(Some(MovementStep {
                next: Vec3::new(current.x + dx * scale, current.y + dy * scale, current.z),
                remaining: horizontal,
            }))
        }
        LocomotionMode::Flight => {
            let distance = current.distance(destination);
            if distance <= acceptable_range {
                return Ok(None);
            }
            let step = maximum_step.min((distance - acceptable_range).max(0.25));
            let scale = step / distance.max(0.001);
            Ok(Some(MovementStep {
                next: Vec3::new(
                    current.x + (destination.x - current.x) * scale,
                    current.y + (destination.y - current.y) * scale,
                    current.z + (destination.z - current.z) * scale,
                ),
                remaining: distance,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_planning_admission_is_bounded_across_controller_clones() {
        let maps_dir = std::env::temp_dir().join(format!(
            "wow-navigation-controller-test-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        std::fs::create_dir_all(&maps_dir).unwrap();
        let terrain = TerrainSampler::new(&maps_dir).unwrap();
        let controller = MovementController::new(terrain);
        let clone = controller.clone();
        let permit = controller.try_acquire_route_planning_slot().unwrap();
        assert_eq!(
            clone.try_acquire_route_planning_slot().unwrap_err(),
            NavigationError::RoutePlannerBusy
        );
        drop(permit);
        assert!(clone.try_acquire_route_planning_slot().is_ok());
        std::fs::remove_dir_all(maps_dir).unwrap();
    }

    #[test]
    fn ground_does_not_invent_z_without_terrain() {
        let s = next_step(
            Vec3::new(0.0, 0.0, 10.0),
            Vec3::new(10.0, 0.0, 30.0),
            1.5,
            0.0,
            LocomotionMode::Ground,
        )
        .unwrap()
        .unwrap();
        assert_eq!(s.next.z, 10.0);
    }
    #[test]
    fn flight_advances_z() {
        let s = next_step(
            Vec3::new(0.0, 0.0, 10.0),
            Vec3::new(10.0, 0.0, 30.0),
            1.5,
            0.0,
            LocomotionMode::Flight,
        )
        .unwrap()
        .unwrap();
        assert!(s.next.z > 10.0);
    }
    #[test]
    fn flight_needs_server_authorized_controlled_mover() {
        assert_eq!(
            LocomotionMode::from_server_flags(true, MOVEMENTFLAG_FLYING),
            LocomotionMode::Flight
        );
        assert_eq!(
            LocomotionMode::from_server_flags(false, MOVEMENTFLAG_FLYING),
            LocomotionMode::Ground
        );
    }
    #[test]
    fn dense_collinear_route_points_are_skipped() {
        assert!(should_skip_dense_collinear_waypoint(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.25, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0)
        ));
    }
    #[test]
    fn route_corner_is_not_cut_by_lookahead() {
        assert!(!should_skip_dense_collinear_waypoint(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0)
        ));
    }
    #[test]
    fn routed_height_interpolates_on_navmesh_segment() {
        let z = route_segment_height(
            Vec3::new(0.0, 0.0, 10.0),
            Vec3::new(10.0, 0.0, 20.0),
            Vec3::new(2.5, 0.0, 10.0),
        );
        assert!((z - 12.5).abs() < 0.001);
    }
    #[test]
    fn routed_height_preserves_stacked_floor_instead_of_raw_terrain() {
        let z = route_segment_height(
            Vec3::new(0.0, 0.0, -30.0),
            Vec3::new(10.0, 0.0, -28.0),
            Vec3::new(5.0, 0.0, -30.0),
        );
        assert!((z + 29.0).abs() < 0.001);
    }
}
