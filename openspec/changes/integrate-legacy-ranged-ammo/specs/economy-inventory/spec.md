### Requirement: Ranged ammo is compatible and replenished safely
The system SHALL derive ranged ammunition compatibility from the authoritative item template in equipment slot 17. It SHALL require authoritative ranged weapon metadata, an authoritative inventory-instance snapshot, and metadata for occupied backpack items before it assumes that the player has no compatible ammunition. Compatible ammo SHALL have item class 6, a subclass equal to the ranged weapon AmmoType, a required level no higher than the player level, and class eligibility.

The system SHALL derive a minimum reserve of five minutes of shots and a target reserve of fifteen minutes from the ranged weapon delay. When the compatible backpack count is below target, maintenance MAY buy one server-defined lot from an already nearby trusted vendor only when the current offer has fixed copper cost, sufficient stock, known compatible item metadata, and leaves at least 1,000 copper. It SHALL use the existing bounded inventory-change confirmation before another maintenance purchase. It SHALL NOT travel to a vendor or buy ammunition from the Auction House.

The system SHALL set the selected ammo only after an authoritative inventory snapshot proves that a compatible, level-eligible stack is present. It SHALL bound repeated ammo-selection requests when the protocol does not report the selected ammo entry.

#### Scenario: Low compatible ammo beside a trusted seller
- **GIVEN** the equipped ranged weapon and backpack inventory are authoritative
- **AND** compatible ammo is below the fifteen-minute target
- **WHEN** a nearby trusted seller has a current fixed-price, compatible offer with one lot in stock and enough funds remain above the reserve
- **THEN** maintenance buys one lot and waits for authoritative inventory change or a bounded timeout before another purchase

#### Scenario: Ammo evidence is incomplete or unsafe
- **WHEN** the ranged item, occupied backpack item metadata, inventory snapshot, offer stock, price, ammo subclass, level eligibility, class eligibility, or reserve evidence is unknown or unsafe
- **THEN** maintenance waits and does not buy

#### Scenario: Selected ammo must be in inventory
- **WHEN** maintenance requests ammo selection
- **THEN** final validation requires an authoritative bag instance and template that match the currently equipped ranged weapon and player level
