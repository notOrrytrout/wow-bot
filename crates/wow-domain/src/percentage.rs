use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Percentage(pub f32);

impl Percentage {
    pub fn clamped(value: f32) -> Self { Self(value.clamp(0.0, 1.0)) }
    pub fn ratio(numerator: u64, denominator: u64) -> Self {
        if denominator == 0 { return Self(0.0); }
        Self::clamped(numerator as f32 / denominator as f32)
    }
}

#[cfg(test)]
mod tests { use super::*; #[test] fn zero_denominator_is_zero(){assert_eq!(Percentage::ratio(10,0),Percentage(0.0));} #[test] fn ratio_clamps(){assert_eq!(Percentage::ratio(2,1),Percentage(1.0));} }
