### Requirement: Named Grind selects a safe nearby exact target
When a named Grind mission selects a voluntary target, it SHALL select only a live hostile unit whose trimmed name matches the mission name exactly without regard to case. It SHALL use a current finite same-map position within 100 yards and choose the nearest candidate, breaking equal-distance ties by entity ID. It SHALL reject a target more than two levels above the player only when both levels are known. It SHALL NOT reject a target solely because target level is unknown.

Known player health below 45 percent or known mana below 20 percent for a mana user SHALL block a voluntary pull. A target with three or more other live hostile units within nine yards SHALL be rejected. A smaller cluster SHALL require at least 75 percent health and 45 percent mana when the respective values are known. Unknown health, mana, or maximum values alone SHALL NOT reject a pull.

Grind SHALL NOT initiate a voluntary pull while the player is observed in combat or when an observed hostile is attacking the player or an online group member. It SHALL leave that work to combat policy. Grind SHALL NOT select player entities.

The current entity model does not expose elite/boss rank or observation timestamps. Grind SHALL NOT infer either fact from entity name or unrelated flags. It SHALL use only entities retained in current authoritative state with usable current position evidence. Until the model adds rank and age, Grind cannot independently exclude elite/boss targets or measure observation freshness.

#### Scenario: Exact target selection is local and deterministic
- **GIVEN** several live hostile units match the requested creature name
- **WHEN** their positions are finite, on the player's map, and within 100 yards
- **THEN** Grind selects the nearest one and uses entity ID to break distance ties
- **AND** it ignores loosely similar names, unavailable positions, wrong-map positions, and targets beyond the range bound

#### Scenario: Missing level or creature rank is not invented
- **GIVEN** a live exact-name target has an unknown level and the current entity model has no creature-rank data
- **WHEN** other voluntary-pull safety gates pass
- **THEN** Grind may select the target
- **AND** it does not claim that the target is non-elite or non-boss

#### Scenario: Low reserves or a dangerous cluster defer Grind
- **WHEN** known health is below 45 percent or known mana is below 20 percent
- **THEN** Grind does not start a voluntary pull
- **WHEN** at least three other hostiles are within nine yards of the target
- **THEN** Grind rejects the target
- **WHEN** one or two other hostiles are within nine yards and known reserves are below 75 percent health or 45 percent mana
- **THEN** Grind rejects the target

#### Scenario: Existing combat retains authority
- **GIVEN** the player is in combat or an observed hostile attacks the player or an online group member
- **WHEN** a named Grind mission is active
- **THEN** Grind does not select another voluntary target
- **AND** existing combat policy remains responsible for self-defense
