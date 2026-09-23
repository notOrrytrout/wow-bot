//! Navigation format responsibilities.

#[allow(unused_imports)]
use super::cache::*;
#[allow(unused_imports)]
use super::graph::*;
#[allow(unused_imports)]
use super::route::*;
#[allow(unused_imports)]
use super::*;

pub(super) const MMAP_MAGIC: u32 = 0x4d4d_4150;
pub(super) const MMAP_HEADER_SIZE: usize = 56;
pub(super) const MMAP_VERSION_MIN: u32 = 19;
pub(super) const MMAP_VERSION_MAX: u32 = 20;
pub(super) const DETOUR_NAVMESH_MAGIC: u32 = 0x444e_4156;
pub(super) const DETOUR_VERSION: u32 = 7;
pub(super) const MAX_TILE_BYTES: usize = 32 * 1024 * 1024;
pub(super) const DEFAULT_CACHE_TILES: usize = 32;
pub(super) const MAX_POLYGONS_PER_TILE: usize = 1 << 16;
pub(super) const MAX_VERTICES_PER_TILE: usize = 1 << 18;
pub(super) const MAX_DETAIL_TRIANGLES: usize = 1 << 20;
pub(super) const VERTICES_PER_POLYGON: usize = 6;
// AzerothCore builds Detour with 64-bit polygon references. Native dtLink is
// therefore 16 bytes after alignment, and this size is part of the tile format.
pub(super) const LINK_BYTES: usize = 16;
pub(super) const EXTERNAL_LINK: u16 = 0x8000;
pub(super) const NAV_GROUND: u16 = 0x01;
pub(super) const NAV_WATER: u16 = 0x08;
pub(super) const WOW_GRID_SIZE: f32 = 533.333_3;
pub(super) const MAX_ROUTE_TILES: usize = 36;
pub(super) const PORTAL_EPSILON: f32 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetourPoint {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl DetourPoint {
    pub fn from_wow(x: f32, y: f32, z: f32) -> Result<Self> {
        if !x.is_finite() || !y.is_finite() || !z.is_finite() {
            bail!("navigation position is not finite");
        }
        Ok(Self { x: y, y: z, z: x })
    }

    pub fn to_wow(self) -> (f32, f32, f32) {
        (self.z, self.x, self.y)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NavMeshParams {
    pub origin: [f32; 3],
    pub tile_width: f32,
    pub tile_height: f32,
    pub max_tiles: u32,
    pub max_polys: u32,
}

impl NavMeshParams {
    pub(super) fn parse(bytes: &[u8]) -> Result<Self> {
        let mut reader = PacketReader::new(bytes);
        let value = Self {
            origin: [reader.f32()?, reader.f32()?, reader.f32()?],
            tile_width: reader.f32()?,
            tile_height: reader.f32()?,
            max_tiles: reader.u32()?,
            max_polys: reader.u32()?,
        };
        if bytes.len() != 28
            || !value.origin.iter().all(|number| number.is_finite())
            || !value.tile_width.is_finite()
            || !value.tile_height.is_finite()
            || value.tile_width <= 0.0
            || value.tile_height <= 0.0
            || value.max_tiles == 0
            || value.max_polys == 0
        {
            bail!("mmap parameters are invalid");
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MmapTile {
    pub mmap_version: u32,
    pub uses_liquids: bool,
    pub walkable_slope_angle: f32,
    pub walkable_radius: u8,
    pub walkable_height: u8,
    pub walkable_climb: u8,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DetourTileHeader {
    pub x: i32,
    pub y: i32,
    pub layer: i32,
    pub user_id: u32,
    pub walkable_height: f32,
    pub walkable_radius: f32,
    pub walkable_climb: f32,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavPolygon {
    pub vertices: Vec<u16>,
    pub neighbors: Vec<u16>,
    pub flags: u16,
    pub area: u8,
    pub polygon_type: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailMesh {
    pub vertex_base: u32,
    pub triangle_base: u32,
    pub vertex_count: u8,
    pub triangle_count: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OffMeshConnection {
    pub start: DetourPoint,
    pub end: DetourPoint,
    pub radius: f32,
    pub polygon: u16,
    pub bidirectional: bool,
    pub side: u8,
    pub user_id: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DetourTile {
    pub header: DetourTileHeader,
    pub vertices: Vec<[f32; 3]>,
    pub polygons: Vec<NavPolygon>,
    pub detail_meshes: Vec<DetailMesh>,
    pub detail_vertices: Vec<[f32; 3]>,
    pub detail_triangles: Vec<[u8; 4]>,
    pub off_mesh_connections: Vec<OffMeshConnection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct PolygonRef {
    pub(super) tile: usize,
    pub(super) polygon: usize,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct GraphOpenNode {
    pub(super) polygon: PolygonRef,
    pub(super) estimated_total: f32,
}

impl PartialEq for GraphOpenNode {
    fn eq(&self, other: &Self) -> bool {
        self.polygon == other.polygon && self.estimated_total == other.estimated_total
    }
}

impl Eq for GraphOpenNode {}

impl PartialOrd for GraphOpenNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for GraphOpenNode {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .estimated_total
            .total_cmp(&self.estimated_total)
            .then_with(|| other.polygon.tile.cmp(&self.polygon.tile))
            .then_with(|| other.polygon.polygon.cmp(&self.polygon.polygon))
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct GraphEdge {
    pub(super) to: PolygonRef,
    pub(super) portal: [DetourPoint; 2],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolygonPoint {
    pub polygon: usize,
    pub point: DetourPoint,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct OpenNode {
    pub(super) polygon: usize,
    pub(super) estimated_total: f32,
}

impl PartialEq for OpenNode {
    fn eq(&self, other: &Self) -> bool {
        self.polygon == other.polygon && self.estimated_total == other.estimated_total
    }
}

impl Eq for OpenNode {}

impl PartialOrd for OpenNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OpenNode {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .estimated_total
            .total_cmp(&self.estimated_total)
            .then_with(|| other.polygon.cmp(&self.polygon))
    }
}

impl MmapTile {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < MMAP_HEADER_SIZE {
            bail!("mmtile header is truncated");
        }
        let mut reader = PacketReader::new(bytes);
        if reader.u32()? != MMAP_MAGIC {
            bail!("mmtile magic is invalid");
        }
        let detour_version = reader.u32()?;
        if detour_version != DETOUR_VERSION {
            bail!("mmtile Detour version {detour_version} is unsupported");
        }
        let mmap_version = reader.u32()?;
        if !(MMAP_VERSION_MIN..=MMAP_VERSION_MAX).contains(&mmap_version) {
            bail!("mmtile version {mmap_version} is unsupported");
        }
        let size = usize::try_from(reader.u32()?).context("mmtile size does not fit usize")?;
        if size == 0 || size > MAX_TILE_BYTES {
            bail!("mmtile payload size is invalid");
        }
        let uses_liquids = reader.u8()? != 0;
        reader.skip(3)?;
        let walkable_slope_angle = reader.f32()?;
        let walkable_radius = reader.u8()?;
        let walkable_height = reader.u8()?;
        let walkable_climb = reader.u8()?;
        reader.skip(1)?;
        reader.skip(28)?;
        if !walkable_slope_angle.is_finite() || !(0.0..=90.0).contains(&walkable_slope_angle) {
            bail!("mmtile walkable slope is invalid");
        }
        let expected = MMAP_HEADER_SIZE
            .checked_add(size)
            .context("mmtile length overflow")?;
        if bytes.len() != expected {
            bail!("mmtile payload length does not match its header");
        }
        let payload = reader.take(size)?.to_vec();
        let mut payload_reader = PacketReader::new(&payload);
        if payload_reader.u32()? != DETOUR_NAVMESH_MAGIC {
            bail!("mmtile Detour payload magic is invalid");
        }
        Ok(Self {
            mmap_version,
            uses_liquids,
            walkable_slope_angle,
            walkable_radius,
            walkable_height,
            walkable_climb,
            payload,
        })
    }

    pub fn detour_tile(&self) -> Result<DetourTile> {
        DetourTile::parse(&self.payload)
    }
}

impl DetourTile {
    pub(super) fn parse(bytes: &[u8]) -> Result<Self> {
        let mut reader = PacketReader::new(bytes);
        if reader.u32()? != DETOUR_NAVMESH_MAGIC || reader.u32()? != DETOUR_VERSION {
            bail!("Detour tile header is incompatible");
        }
        let x = reader.i32()?;
        let y = reader.i32()?;
        let layer = reader.i32()?;
        let user_id = reader.u32()?;
        let poly_count = checked_count(reader.i32()?, MAX_POLYGONS_PER_TILE, "polygon")?;
        let vertex_count = checked_count(reader.i32()?, MAX_VERTICES_PER_TILE, "vertex")?;
        let max_link_count = checked_count(reader.i32()?, 1 << 20, "link")?;
        let detail_mesh_count = checked_count(reader.i32()?, MAX_POLYGONS_PER_TILE, "detail mesh")?;
        let detail_vertex_count =
            checked_count(reader.i32()?, MAX_VERTICES_PER_TILE, "detail vertex")?;
        let detail_triangle_count =
            checked_count(reader.i32()?, MAX_DETAIL_TRIANGLES, "detail triangle")?;
        let bv_node_count = checked_count(reader.i32()?, 1 << 20, "BV node")?;
        let off_mesh_count = checked_count(reader.i32()?, 1 << 16, "off-mesh connection")?;
        let off_mesh_base = checked_count(reader.i32()?, poly_count, "off-mesh base")?;
        let walkable_height = finite_nonnegative(reader.f32()?, "walkable height")?;
        let walkable_radius = finite_nonnegative(reader.f32()?, "walkable radius")?;
        let walkable_climb = finite_nonnegative(reader.f32()?, "walkable climb")?;
        let bounds_min = read_point(&mut reader).context("invalid Detour minimum bounds")?;
        let bounds_max = read_point(&mut reader).context("invalid Detour maximum bounds")?;
        let bv_quantization = reader.f32()?;
        if !bv_quantization.is_finite() || bv_quantization < 0.0 {
            bail!("Detour BV quantization is invalid");
        }
        if detail_mesh_count != off_mesh_base || off_mesh_base > poly_count {
            bail!("Detour tile section counts are inconsistent");
        }
        for axis in 0..3 {
            if bounds_min[axis] > bounds_max[axis] {
                bail!("Detour tile bounds are inverted");
            }
        }

        let mut vertices = Vec::with_capacity(vertex_count);
        for index in 0..vertex_count {
            vertices.push(
                read_point(&mut reader)
                    .with_context(|| format!("invalid Detour vertex {index}"))?,
            );
        }

        let mut polygons = Vec::with_capacity(poly_count);
        for _ in 0..poly_count {
            reader.u32()?; // Runtime link-list head. Serialized tiles use no stable link index.
            let mut all_vertices = [0u16; VERTICES_PER_POLYGON];
            for value in &mut all_vertices {
                *value = reader.u16()?;
            }
            let mut all_neighbors = [0u16; VERTICES_PER_POLYGON];
            for value in &mut all_neighbors {
                *value = reader.u16()?;
            }
            let flags = reader.u16()?;
            let polygon_vertex_count = usize::from(reader.u8()?);
            let area_and_type = reader.u8()?;
            if !(2..=VERTICES_PER_POLYGON).contains(&polygon_vertex_count) {
                bail!("Detour polygon vertex count is invalid");
            }
            let polygon_vertices = all_vertices[..polygon_vertex_count].to_vec();
            if polygon_vertices
                .iter()
                .any(|index| usize::from(*index) >= vertex_count)
            {
                bail!("Detour polygon references an invalid vertex");
            }
            polygons.push(NavPolygon {
                vertices: polygon_vertices,
                neighbors: all_neighbors[..polygon_vertex_count].to_vec(),
                flags,
                area: area_and_type & 0x3f,
                polygon_type: area_and_type >> 6,
            });
        }

        checked_skip(&mut reader, max_link_count, LINK_BYTES, "links")?;

        let mut detail_meshes = Vec::with_capacity(detail_mesh_count);
        for _ in 0..detail_mesh_count {
            let mesh = DetailMesh {
                vertex_base: reader.u32()?,
                triangle_base: reader.u32()?,
                vertex_count: reader.u8()?,
                triangle_count: reader.u8()?,
            };
            reader.skip(2)?; // Native structure padding.
            let vertex_end = usize::try_from(mesh.vertex_base)?
                .checked_add(usize::from(mesh.vertex_count))
                .context("detail vertex range overflow")?;
            let triangle_end = usize::try_from(mesh.triangle_base)?
                .checked_add(usize::from(mesh.triangle_count))
                .context("detail triangle range overflow")?;
            if vertex_end > detail_vertex_count || triangle_end > detail_triangle_count {
                bail!("Detour detail mesh range is invalid");
            }
            detail_meshes.push(mesh);
        }

        let mut detail_vertices = Vec::with_capacity(detail_vertex_count);
        for index in 0..detail_vertex_count {
            detail_vertices.push(
                read_point(&mut reader)
                    .with_context(|| format!("invalid Detour detail vertex {index}"))?,
            );
        }
        let mut detail_triangles = Vec::with_capacity(detail_triangle_count);
        for _ in 0..detail_triangle_count {
            detail_triangles.push(reader.take(4)?.try_into().unwrap());
        }
        checked_skip(&mut reader, bv_node_count, 16, "BV nodes")?;
        let mut off_mesh_connections = Vec::with_capacity(off_mesh_count);
        for _ in 0..off_mesh_count {
            let start = DetourPoint {
                x: reader.f32()?,
                y: reader.f32()?,
                z: reader.f32()?,
            };
            let end = DetourPoint {
                x: reader.f32()?,
                y: reader.f32()?,
                z: reader.f32()?,
            };
            let radius = finite_nonnegative(reader.f32()?, "off-mesh radius")?;
            let polygon = reader.u16()?;
            let bidirectional = reader.u8()? & 1 != 0;
            let side = reader.u8()?;
            let user_id = reader.u32()?;
            if ![start, end]
                .iter()
                .all(|point| point.x.is_finite() && point.y.is_finite() && point.z.is_finite())
                || usize::from(polygon) >= poly_count
            {
                bail!("Detour off-mesh connection is invalid");
            }
            off_mesh_connections.push(OffMeshConnection {
                start,
                end,
                radius,
                polygon,
                bidirectional,
                side,
                user_id,
            });
        }
        if reader.remaining() != 0 {
            bail!("Detour tile has unexpected trailing bytes");
        }
        Ok(Self {
            header: DetourTileHeader {
                x,
                y,
                layer,
                user_id,
                walkable_height,
                walkable_radius,
                walkable_climb,
                bounds_min,
                bounds_max,
            },
            vertices,
            polygons,
            detail_meshes,
            detail_vertices,
            detail_triangles,
            off_mesh_connections,
        })
    }

    pub fn nearest_polygon(
        &self,
        point: DetourPoint,
        horizontal_extent: f32,
        vertical_extent: f32,
        allow_water: bool,
    ) -> Option<PolygonPoint> {
        if !horizontal_extent.is_finite()
            || !vertical_extent.is_finite()
            || horizontal_extent < 0.0
            || vertical_extent < 0.0
        {
            return None;
        }
        self.polygons
            .iter()
            .enumerate()
            .filter(|(_, polygon)| polygon.walkable(allow_water))
            .filter_map(|(index, _)| {
                let closest = self.closest_point_on_polygon(index, point)?;
                let dx = closest.x - point.x;
                let dz = closest.z - point.z;
                let dy = (closest.y - point.y).abs();
                (dx.abs() <= horizontal_extent
                    && dz.abs() <= horizontal_extent
                    && dy <= vertical_extent)
                    .then_some((dx * dx + dz * dz + dy * dy, index, closest))
            })
            .min_by(|left, right| left.0.total_cmp(&right.0))
            .map(|(_, polygon, point)| PolygonPoint { polygon, point })
    }

    pub fn find_polygon_path(
        &self,
        start: usize,
        end: usize,
        allow_water: bool,
        max_visited: usize,
    ) -> Result<Vec<usize>> {
        if start >= self.polygons.len() || end >= self.polygons.len() {
            bail!("navigation path endpoint polygon is invalid");
        }
        if !self.polygons[start].walkable(allow_water) || !self.polygons[end].walkable(allow_water)
        {
            bail!("navigation path endpoint is not walkable");
        }
        let limit = max_visited.clamp(1, self.polygons.len());
        let end_center = self.polygon_center(end)?;
        let mut open = BinaryHeap::new();
        let mut cost = vec![f32::INFINITY; self.polygons.len()];
        let mut previous = vec![None; self.polygons.len()];
        cost[start] = 0.0;
        open.push(OpenNode {
            polygon: start,
            estimated_total: distance3(self.polygon_center(start)?, end_center),
        });
        let mut visited = 0usize;
        while let Some(node) = open.pop() {
            visited += 1;
            if visited > limit {
                return Err(NavigationError::SearchLimit {
                    expanded_nodes: visited,
                    limit,
                }
                .into());
            }
            if node.polygon == end {
                let mut path = vec![end];
                let mut current = end;
                while current != start {
                    current =
                        previous[current].context("navigation path predecessor is missing")?;
                    path.push(current);
                }
                path.reverse();
                return Ok(path);
            }
            let current_center = self.polygon_center(node.polygon)?;
            for neighbor in self.internal_neighbors(node.polygon) {
                if !self.polygons[neighbor].walkable(allow_water) {
                    continue;
                }
                let neighbor_center = self.polygon_center(neighbor)?;
                let next_cost = cost[node.polygon] + distance3(current_center, neighbor_center);
                if next_cost < cost[neighbor] {
                    cost[neighbor] = next_cost;
                    previous[neighbor] = Some(node.polygon);
                    open.push(OpenNode {
                        polygon: neighbor,
                        estimated_total: next_cost + distance3(neighbor_center, end_center),
                    });
                }
            }
        }
        Err(NavigationError::Disconnected { explored: None }.into())
    }

    pub(super) fn internal_neighbors(&self, polygon: usize) -> impl Iterator<Item = usize> + '_ {
        self.polygons[polygon]
            .neighbors
            .iter()
            .copied()
            .filter(|neighbor| *neighbor != 0 && *neighbor & EXTERNAL_LINK == 0)
            .filter_map(|neighbor| usize::from(neighbor).checked_sub(1))
            .filter(|neighbor| *neighbor < self.polygons.len())
    }

    pub(super) fn polygon_center(&self, polygon: usize) -> Result<DetourPoint> {
        let polygon = self
            .polygons
            .get(polygon)
            .context("navigation polygon is unavailable")?;
        let mut center = [0.0; 3];
        for index in &polygon.vertices {
            let vertex = self
                .vertices
                .get(usize::from(*index))
                .context("navigation polygon vertex is unavailable")?;
            for axis in 0..3 {
                center[axis] += vertex[axis];
            }
        }
        let divisor = polygon.vertices.len() as f32;
        Ok(DetourPoint {
            x: center[0] / divisor,
            y: center[1] / divisor,
            z: center[2] / divisor,
        })
    }

    pub(super) fn closest_point_on_polygon(
        &self,
        polygon: usize,
        point: DetourPoint,
    ) -> Option<DetourPoint> {
        let vertices = self.polygon_points(polygon)?;
        let mut closest = point;
        if !point_in_polygon_xz(point, &vertices) {
            let (_, edge_point) = vertices
                .iter()
                .zip(vertices.iter().cycle().skip(1))
                .map(|(start, end)| {
                    let candidate = closest_on_segment_xz(point, *start, *end);
                    let dx = candidate.x - point.x;
                    let dz = candidate.z - point.z;
                    (dx * dx + dz * dz, candidate)
                })
                .min_by(|left, right| left.0.total_cmp(&right.0))?;
            closest.x = edge_point.x;
            closest.z = edge_point.z;
        }
        closest.y = self.surface_height(polygon, closest.x, closest.z)?;
        Some(closest)
    }

    pub(super) fn polygon_points(&self, polygon: usize) -> Option<Vec<DetourPoint>> {
        self.polygons
            .get(polygon)?
            .vertices
            .iter()
            .map(|index| {
                self.vertices
                    .get(usize::from(*index))
                    .map(|value| DetourPoint {
                        x: value[0],
                        y: value[1],
                        z: value[2],
                    })
            })
            .collect()
    }

    pub(super) fn surface_height(&self, polygon: usize, x: f32, z: f32) -> Option<f32> {
        let nav_polygon = self.polygons.get(polygon)?;
        let detail = self.detail_meshes.get(polygon)?;
        for triangle_offset in 0..usize::from(detail.triangle_count) {
            let triangle = self.detail_triangles.get(
                usize::try_from(detail.triangle_base)
                    .ok()?
                    .checked_add(triangle_offset)?,
            )?;
            let points: Option<Vec<_>> = triangle[..3]
                .iter()
                .map(|index| {
                    let index = usize::from(*index);
                    if index < nav_polygon.vertices.len() {
                        self.vertices
                            .get(usize::from(nav_polygon.vertices[index]))
                            .copied()
                    } else {
                        let detail_index = usize::try_from(detail.vertex_base)
                            .ok()?
                            .checked_add(index.checked_sub(nav_polygon.vertices.len())?)?;
                        self.detail_vertices.get(detail_index).copied()
                    }
                })
                .collect();
            let points = points?;
            if let Some(height) = triangle_height_xz(x, z, points[0], points[1], points[2]) {
                return Some(height);
            }
        }
        let points = self.polygon_points(polygon)?;
        Some(points.iter().map(|value| value.y).sum::<f32>() / points.len() as f32)
    }
}

impl NavPolygon {
    pub(super) fn walkable(&self, allow_water: bool) -> bool {
        self.polygon_type == 0
            && (self.flags & NAV_GROUND != 0 || (allow_water && self.flags & NAV_WATER != 0))
    }
}

pub(super) fn distance3(left: DetourPoint, right: DetourPoint) -> f32 {
    ((left.x - right.x).powi(2) + (left.y - right.y).powi(2) + (left.z - right.z).powi(2)).sqrt()
}

pub(super) fn point_in_polygon_xz(point: DetourPoint, polygon: &[DetourPoint]) -> bool {
    let mut inside = false;
    for (start, end) in polygon.iter().zip(polygon.iter().cycle().skip(1)) {
        if (start.z > point.z) != (end.z > point.z)
            && point.x < (end.x - start.x) * (point.z - start.z) / (end.z - start.z) + start.x
        {
            inside = !inside;
        }
    }
    inside
}

pub(super) fn closest_on_segment_xz(
    point: DetourPoint,
    start: DetourPoint,
    end: DetourPoint,
) -> DetourPoint {
    let dx = end.x - start.x;
    let dz = end.z - start.z;
    let length_squared = dx * dx + dz * dz;
    let factor = if length_squared <= f32::EPSILON {
        0.0
    } else {
        ((point.x - start.x) * dx + (point.z - start.z) * dz) / length_squared
    }
    .clamp(0.0, 1.0);
    DetourPoint {
        x: start.x + dx * factor,
        y: start.y + (end.y - start.y) * factor,
        z: start.z + dz * factor,
    }
}

pub(super) fn triangle_height_xz(
    x: f32,
    z: f32,
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
) -> Option<f32> {
    let denominator = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
    if denominator.abs() <= f32::EPSILON {
        return None;
    }
    let first = ((b[2] - c[2]) * (x - c[0]) + (c[0] - b[0]) * (z - c[2])) / denominator;
    let second = ((c[2] - a[2]) * (x - c[0]) + (a[0] - c[0]) * (z - c[2])) / denominator;
    let third = 1.0 - first - second;
    (first >= -0.001 && second >= -0.001 && third >= -0.001)
        .then_some(first * a[1] + second * b[1] + third * c[1])
}

pub(super) fn checked_count(value: i32, limit: usize, name: &str) -> Result<usize> {
    let value =
        usize::try_from(value).with_context(|| format!("Detour {name} count is negative"))?;
    if value > limit {
        bail!("Detour {name} count exceeds {limit}");
    }
    Ok(value)
}

pub(super) fn checked_skip(
    reader: &mut PacketReader<'_>,
    count: usize,
    size: usize,
    name: &str,
) -> Result<()> {
    let length = count
        .checked_mul(size)
        .with_context(|| format!("Detour {name} section overflows"))?;
    reader.skip(length)?;
    Ok(())
}

pub(super) fn finite_nonnegative(value: f32, name: &str) -> Result<f32> {
    if !value.is_finite() || value < 0.0 {
        bail!("Detour {name} is invalid");
    }
    Ok(value)
}

pub(super) fn read_point(reader: &mut PacketReader<'_>) -> Result<[f32; 3]> {
    let point = [reader.f32()?, reader.f32()?, reader.f32()?];
    if !point.iter().all(|value| value.is_finite()) {
        bail!("Detour point is not finite");
    }
    Ok(point)
}
