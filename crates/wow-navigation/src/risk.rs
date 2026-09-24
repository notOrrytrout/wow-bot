use wow_domain::Vec3;
#[derive(Clone, Debug)]
pub struct RiskZone {
    pub center: Vec3,
    pub radius: f32,
    pub weight: f32,
}
pub fn penalty(p: Vec3, zones: &[RiskZone]) -> f32 {
    zones
        .iter()
        .filter(|z| p.distance(z.center) <= z.radius)
        .map(|z| z.weight)
        .sum()
}
pub fn materially_safer(
    current: f32,
    candidate: f32,
    min_improvement: f32,
    immediate_danger: bool,
) -> bool {
    immediate_danger || current - candidate >= min_improvement
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn small_difference_does_not_oscillate() {
        assert!(!materially_safer(10.0, 9.8, 1.0, false));
        assert!(materially_safer(10.0, 8.5, 1.0, false));
    }
}
