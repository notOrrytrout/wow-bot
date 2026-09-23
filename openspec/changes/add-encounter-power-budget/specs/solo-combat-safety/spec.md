## ADDED Requirements

### Requirement: Solo encounter power budgeting

The system SHALL estimate solo encounter risk by comparing the player's effective power against the observed hostile pack power. The estimate SHALL include player level, hostile levels, hostile count, elite or boss rank evidence, available equipment-condition evidence, health/resource pressure, role/profile survivability, and uncertainty.

#### Scenario: Higher-level player faces low-level pack
- **WHEN** a level 25 solo bot observes three level 10 normal hostile creatures
- **THEN** the bot does not flee only because hostile count is high

#### Scenario: Lower-level melee player faces higher-level pack
- **WHEN** a level 15 solo melee bot observes three level 25 hostile creatures
- **THEN** the encounter-power budget classifies the pack as unsafe enough for flee or avoidance

#### Scenario: Elite rank increases danger
- **WHEN** a hostile creature is observed as elite, rare-elite, or boss-ranked
- **THEN** the hostile pack power increases relative to a normal creature of the same level

#### Scenario: Gear signal is unavailable
- **WHEN** no reliable equipment-condition signal is available
- **THEN** the gear contribution is neutral and the risk estimate records additional uncertainty

### Requirement: Solo ranged conservatism and spacing

The system SHALL keep solo ranged/caster profiles more conservative than comparable melee profiles and SHALL preserve ranged spacing behavior after the opening shot. The system SHALL NOT apply ranged kite-back behavior to melee profiles.

#### Scenario: Ranged and melee compare equal pack power
- **WHEN** a solo ranged/caster profile and a solo melee profile face the same comparable hostile pack
- **THEN** the ranged/caster profile receives stricter survival treatment than the melee profile

#### Scenario: Melee profile remains melee
- **WHEN** a melee-specialized profile enters combat
- **THEN** it does not use the solo ranged kite-back rule

### Requirement: Recent killer risk memory

The system SHALL remember a bounded set of creature entries and locations that recently killed the bot and SHALL increase encounter risk when the bot later observes matching hostile creatures near the remembered death area. The memory SHALL expire and SHALL NOT become durable global tombstones.

#### Scenario: Recently lethal creature appears again
- **WHEN** a bot observes a hostile creature entry near a recent death-risk memory for the same bot and map
- **THEN** the encounter-power budget increases hostile pack power for that creature

#### Scenario: Old lethal memory expires
- **WHEN** the remembered death-risk record is expired
- **THEN** it does not increase current encounter risk
