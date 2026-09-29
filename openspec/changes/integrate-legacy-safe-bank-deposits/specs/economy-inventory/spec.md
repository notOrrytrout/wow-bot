## ADDED Requirements

### Requirement: Safe bank deposits under severe bag pressure

Maintenance SHALL deposit only known profession materials from the base backpack when authoritative inventory state shows two or fewer free slots. It SHALL not perform bank work outside the world, during combat, during active casts, while dead or a ghost, while moving, with a controlled mover, or while attached to a transport. It SHALL require a nearby observed interactable banker and a server-confirmed bank-open observation bound to the same banker before it deposits. It SHALL exclude configured `bank_keep_item_ids`, active quest objective items, and known spell reagent items. Missing inventory instances or item metadata SHALL prevent a deposit. It SHALL deposit one item stack per attempt and wait for an authoritative inventory instance decrease or removal before another deposit. Bank open and deposit waits SHALL use bounded timeouts and delayed retries. Final validation SHALL re-check bank-work eligibility, bag pressure, banker identity and live range, bank-open state, and the exact current item ID, GUID, slot, and profession-material classification. Bank-open state SHALL be cleared on world changes and banker removal. When no banker is nearby, the lane MAY route to a remembered same-map banker location if the observation is no more than 30 minutes old and the location is within 30 yards. The route SHALL preserve the active mission, require maintenance and movement permission, and stop when safety, bag-pressure, combat, cast, controlled-mover, transport, nearby-quest-offer gates fail or a live banker appears. Remembered location SHALL NOT authorize bank activation or deposits.

#### Scenario: Nearby bank can safely free backpack space

- **GIVEN** authoritative inventory state shows two or fewer free slots
- **AND** a known profession material is present in the base backpack with complete item metadata
- **AND** an observed interactable banker with an authoritative banker service flag is within five yards
- **WHEN** the server confirms that this banker opened the bank
- **THEN** maintenance deposits one eligible stack through the validated bank action
- **AND** it waits for an authoritative inventory instance update before another deposit

#### Scenario: Bank or inventory evidence is incomplete or stale

- **WHEN** the banker is not currently observed in range, the bank-open observation identifies another banker, severe bag pressure has cleared, or the selected item instance/template has changed
- **THEN** final validation rejects the deposit

#### Scenario: Item is protected or not known to be a profession material

- **WHEN** an item is a quest objective item, a known spell reagent, has unknown metadata, or has an item class other than profession material
- **THEN** maintenance does not select it for deposit

#### Scenario: Open or deposit confirmation does not arrive

- **WHEN** the server does not confirm bank opening or the authoritative inventory does not confirm a deposit within the bounded wait
- **THEN** maintenance delays its retry and does not repeat the action each tick

#### Scenario: Remembered banker is only a movement destination
- **WHEN** severe bag pressure and a safe candidate exist, but no live banker is nearby
- **AND** recent same-map banker memory is within the short detour radius
- **THEN** the lane may route toward the remembered position without replacing the mission
- **AND** it sends no bank action until it observes a live nearby banker and matching authoritative bank-open state

#### Scenario: A configured keep item is selected for deposit
- **WHEN** the selected item ID is present in `bank_keep_item_ids`
- **THEN** candidate selection and final validation reject the deposit
