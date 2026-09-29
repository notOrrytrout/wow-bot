# Design

The group follow policy owns the role-to-distance mapping. The lane reads the configured role from `MissionIntent::Party` or `MissionIntent::Raid`, then passes the resulting distance to the existing follow destination calculation. Other missions use `Auto` spacing.

The destination calculation uses the full three-dimensional position delta. It continues to reject non-finite positions and map mismatches. Movement still goes through the existing navigation controller and its safety checks.
