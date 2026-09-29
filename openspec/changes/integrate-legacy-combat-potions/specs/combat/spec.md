## ADDED Requirements

### Requirement: Critical combat potions use authoritative inventory and shared lockout
During active combat, the system SHALL use an authoritative backpack health potion when player health is at or below 25 percent and no legal self-heal is available, or at or below 15 percent regardless of self-heal availability. It SHALL use a mana potion only when current combat work remains active and mana is at or below 20 percent for a healer or 15 percent for another role. Potion use SHALL require authoritative backpack instance and item-template data, SHALL be blocked by authoritative Potion Sickness, and SHALL start a shared one-hour fallback lockout for both health and mana potions.

#### Scenario: Health potion fills a critical gap
- **WHEN** the player is in active combat at or below 25 percent health and has no currently legal self-heal
- **AND** an authoritative backpack instance has metadata identifying a usable health potion
- **AND** Potion Sickness is absent and the shared fallback lockout has expired
- **THEN** the lane uses that observed item instance on the player

#### Scenario: Extreme health overrides a ready self-heal
- **WHEN** the player is at or below 15 percent health in active combat
- **AND** an authoritative health potion is present
- **THEN** the lane may use the potion even if a legal self-heal is available

#### Scenario: Mana potion requires ongoing combat work
- **WHEN** a healer has at most 20 percent mana, or another role has at most 15 percent mana
- **AND** active combat work remains
- **AND** an authoritative mana potion is present with no Potion Sickness or shared fallback lockout
- **THEN** the lane may use the potion on the player
- **AND** it does not use a mana potion when no combat work remains

#### Scenario: Potion Sickness and shared fallback lockout prevent chaining
- **WHEN** Potion Sickness is authoritative or a health or mana potion was used within the previous hour
- **THEN** the lane does not use either potion type

#### Scenario: Potion inventory evidence is incomplete
- **WHEN** the backpack instance or item-template metadata is missing
- **THEN** the lane does not infer or use that potion
