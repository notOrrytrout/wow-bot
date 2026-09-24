use std::time::{Duration, Instant};

use super::cache::LocalSurfaceSample;

/// A bounded snapshot of navmesh evidence along a short local movement segment.
/// Missing geometry is unknown and therefore cannot authorize movement.
#[derive(Clone, Debug)]
pub struct LocalGeometrySample {
    sampled_at: Instant,
    pub points: Vec<LocalSurfaceSample>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalRouteAssessment {
    Clear,
    Blocked,
    UnsafeDrop,
    Unknown,
    Stale,
}

impl LocalGeometrySample {
    pub(super) fn new(points: Vec<LocalSurfaceSample>) -> Self {
        Self {
            sampled_at: Instant::now(),
            points,
        }
    }

    /// Only fresh, complete samples with bounded floor changes authorize a step.
    pub fn assess(
        &self,
        now: Instant,
        max_age: Duration,
        max_drop_yards: f32,
    ) -> LocalRouteAssessment {
        if now.saturating_duration_since(self.sampled_at) > max_age {
            return LocalRouteAssessment::Stale;
        }
        if self.points.is_empty() || self.points.iter().any(|point| !point.available) {
            return LocalRouteAssessment::Unknown;
        }
        if self.points.iter().any(|point| point.height.is_none()) {
            return LocalRouteAssessment::Blocked;
        }
        if self
            .points
            .iter()
            .any(|point| point.height.is_some_and(|height| !height.is_finite()))
        {
            return LocalRouteAssessment::Unknown;
        }
        if self.points.windows(2).any(|pair| {
            pair[0]
                .height
                .zip(pair[1].height)
                .is_some_and(|(a, b)| (a - b).abs() > max_drop_yards)
        }) {
            return LocalRouteAssessment::UnsafeDrop;
        }
        LocalRouteAssessment::Clear
    }
}

pub(super) const MAX_LOCAL_VISION_POINTS: usize = 32;

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(heights: &[Option<f32>]) -> LocalGeometrySample {
        LocalGeometrySample::new(
            heights
                .iter()
                .map(|height| LocalSurfaceSample {
                    available: height.is_some(),
                    height: *height,
                    tile: None,
                    polygon: None,
                })
                .collect(),
        )
    }

    #[test]
    fn stale_local_geometry_does_not_authorize_movement() {
        let value = sample(&[Some(2.0), Some(2.0)]);
        assert_eq!(
            value.assess(
                Instant::now() + Duration::from_secs(2),
                Duration::from_secs(1),
                2.0
            ),
            LocalRouteAssessment::Stale
        );
    }

    #[test]
    fn missing_geometry_blocks_and_large_drop_is_unsafe() {
        let hole = LocalGeometrySample::new(vec![
            LocalSurfaceSample {
                available: true,
                height: Some(2.0),
                tile: None,
                polygon: None,
            },
            LocalSurfaceSample {
                available: true,
                height: None,
                tile: None,
                polygon: None,
            },
        ]);
        assert_eq!(
            hole.assess(Instant::now(), Duration::from_secs(1), 2.0),
            LocalRouteAssessment::Blocked
        );
        assert_eq!(
            sample(&[Some(5.0), Some(1.0)]).assess(Instant::now(), Duration::from_secs(1), 2.0),
            LocalRouteAssessment::UnsafeDrop
        );
    }
}
