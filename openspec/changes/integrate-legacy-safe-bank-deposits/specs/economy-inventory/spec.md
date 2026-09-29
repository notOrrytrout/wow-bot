## ADDED Requirements

### Requirement: Safe bank deposits under severe bag pressure

Maintenance SHALL deposit only known profession materials from the base backpack when authoritative inventory state shows two or fewer free slots. It SHALL not perform bank work outside the world, during combat, during active casts, while dead or a ghost, while moving, with a controlled mover, or while attached to a transport. It SHALL require a nearby observed interactable banker and a server-confirmed bank-open observation bound to the same banker before it deposits. It SHALL exclude active quest objective items and known spell reagent items. Missing inventory instances or item metadata SHALL prevent a deposit. It SHALL deposit one item stack per attempt and wait for an authoritative inventory instance decrease or removal before another deposit. Bank open and deposit waits SHALL use bounded timeouts and delayed retries. Final validation SHALL re-check bank-work eligibility, bag pressure, banker identity and live range, bank-open state, and the exact current item ID, GUID, slot, and profession-material classification. Bank-open state SHALL be cleared on world changes and banker removal.

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
