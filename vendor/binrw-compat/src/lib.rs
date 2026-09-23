//! Compatibility bridge for Tentacli's `binrw 0.14` requirement.
//!
//! The 0.15 release keeps the API used by Tentacli and corrects the private
//! crate re-export that produces a future-incompatibility warning in 0.14.2.

pub use binrw_upstream::*;
