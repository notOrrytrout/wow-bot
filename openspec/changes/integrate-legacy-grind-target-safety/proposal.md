# Integrate Legacy Grind Target Safety

Restore bounded voluntary target selection from the legacy Grind mode while preserving the lane architecture.

- Select the nearest exact-name hostile unit from current same-map position observations.
- Apply the legacy level and survival thresholds when current data can support them.
- Keep voluntary Grind from taking over an active fight or an observed self-defense attacker.
- Record the current limits: entity state has no creature rank or observation-age fields.
