use serde::{Deserialize, Serialize};
use wow_domain::Vec3;
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Route {
    pub points: Vec<Vec3>,
    pub cost: f32,
}
impl Route {
    pub fn direct(from: Vec3, to: Vec3) -> Self {
        Self {
            points: vec![from, to],
            cost: from.distance(to),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}
