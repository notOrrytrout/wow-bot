use wow_domain::Vec3;
pub fn same_floor(a: Vec3, b: Vec3, tolerance: f32) -> bool {
    a.is_finite() && b.is_finite() && (a.z - b.z).abs() <= tolerance
}
pub fn accept_surface_correction(current: Vec3, candidate: Vec3, tolerance: f32) -> Option<Vec3> {
    same_floor(current, candidate, tolerance).then_some(candidate)
}
