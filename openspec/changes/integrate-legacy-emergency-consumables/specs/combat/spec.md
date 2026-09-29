## ADDED Requirements

### Requirement: Emergency survival items use authoritative backpack data
During confirmed combat, the system SHALL use an authoritative healthstone instance when player health is at or below 30 percent. It SHALL consider a Whipper Root Tuber or Night Dragon's Breath only when player health is at or below 25 percent and no legal self-heal is available, or at or below 15 percent regardless of self-heal availability. Healthstones SHALL use a separate five-second retry window. The non-potion items SHALL use a separate 30-second retry window and SHALL NOT use or be blocked by Potion Sickness or the shared potion lockout. All uses SHALL require a current backpack instance and authoritative item-template name and use-spell metadata.

#### Scenario: Healthstone is available at the legacy threshold
- **GIVEN** confirmed combat and authoritative player health at or below 30 percent
- **AND** a current backpack instance has item-template metadata for a usable healthstone
- **WHEN** the healthstone retry window has expired
- **THEN** the lane uses that instance on the player

#### Scenario: Non-potion consumable fills a critical health gap
- **GIVEN** confirmed combat and player health at or below 25 percent without a legal self-heal, or at or below 15 percent
- **AND** a current backpack instance has item-template metadata for a usable Whipper Root Tuber or Night Dragon's Breath
- **WHEN** no higher-priority emergency item is selected and its 30-second retry window has expired
- **THEN** the lane uses the observed item instance on the player
- **AND** Potion Sickness and the shared potion lockout do not block this action

#### Scenario: Emergency item evidence is incomplete
- **WHEN** the item instance, backpack location, item name, or use-spell metadata is missing or invalid
- **THEN** the lane does not infer or use the emergency item

#### Scenario: Spirit of Redemption is active
- **WHEN** the player is a Priest with authoritative Spirit of Redemption aura state
- **THEN** the lane does not spend a healthstone, Whipper Root Tuber, or Night Dragon's Breath to preserve the temporary 1-health state
