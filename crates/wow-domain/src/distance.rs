use crate::Vec3;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Yards(pub f32);

impl Yards {
    pub const fn new(value: f32) -> Self {
        Self(value)
    }
    pub fn between(a: Vec3, b: Vec3) -> Option<Self> {
        (a.is_finite() && b.is_finite()).then(|| Self(a.distance(b)))
    }
}
