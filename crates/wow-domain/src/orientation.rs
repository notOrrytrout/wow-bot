use serde::{Deserialize, Serialize};
use std::f32::consts::TAU;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Radians(pub f32);

impl Radians {
    pub fn normalized(value: f32) -> Option<Self> {
        if !value.is_finite() { return None; }
        Some(Self(value.rem_euclid(TAU)))
    }
}

#[cfg(test)]
mod tests { use super::*; #[test] fn normalization_is_canonical(){let a=Radians::normalized(-1.0).unwrap().0;let b=Radians::normalized(std::f32::consts::TAU-1.0).unwrap().0;assert!((a-b).abs()<1e-5);} }
