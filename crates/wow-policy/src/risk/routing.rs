pub fn acceptable(score: f32, limit: f32) -> bool {
    score.is_finite() && score <= limit
}
pub fn should_replace(
    current: f32,
    candidate: f32,
    material_improvement: f32,
    immediate_danger: bool,
) -> bool {
    immediate_danger
        || (current.is_finite()
            && candidate.is_finite()
            && current - candidate >= material_improvement)
}
