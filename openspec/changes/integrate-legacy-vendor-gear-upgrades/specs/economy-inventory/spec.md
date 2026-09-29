## ADDED Requirements

### Requirement: Opportunistic vendor gear upgrades

The system SHALL allow optional maintenance to buy a usable gear upgrade from an already nearby trusted seller. It SHALL NOT travel to find a seller for gear upgrades. It SHALL require authoritative equipment slots and inventory, known class and level, at least one free bag slot, and metadata for all equipped items. It SHALL defer this optional purchase while the player is in combat, attached to a transport, or a known quest offer is within 40 yards.

A candidate SHALL use a current server-observed vendor offer with no extended currency cost, enough stock for one lot or server-reported unlimited stock, and authoritative item metadata. The item SHALL be usable by the player and improve the best applicable equipment slot by at least five percent under the shared deterministic gear score policy. The policy SHALL rank candidates by score gain, then lower price, then lower item ID.

The purchase SHALL cost no more than 25 percent of current money and SHALL preserve the lane's 1,000-copper maintenance reserve. Maintenance SHALL buy one server-defined lot per attempt. The final action validator SHALL re-check the current vendor, offer slot and item, stock, price reserve and wallet cap, item eligibility, and trusted nearby seller.

#### Scenario: Safe nearby offer improves equipped gear

- **GIVEN** authoritative inventory and equipment data show free bag space
- **AND** an eligible vendor is already within interaction range
- **AND** its current cash offer has sufficient stock and item metadata proves at least a five percent upgrade
- **WHEN** the offer price is within the wallet cap and maintenance reserve
- **THEN** maintenance buys one lot through the shared validated vendor action
- **AND** it does not start vendor travel

#### Scenario: Vendor list or item metadata is missing

- **WHEN** a nearby seller has no current offer list or a budget-eligible offer has unknown item metadata
- **THEN** maintenance requests the offer list or item metadata
- **AND** it does not buy until authoritative evidence is available

#### Scenario: Gear purchase is unsafe or optional work is preempted

- **WHEN** inventory or equipment is not authoritative, no bag slot is free, the offer has an extended cost or insufficient finite stock, the candidate is unusable or below the upgrade threshold, the cost exceeds either cap, or a nearby quest offer has priority
- **THEN** maintenance does not buy the offer

#### Scenario: Offer changes before the purchase

- **WHEN** the current offer slot no longer identifies the selected item or the vendor, stock, seller, or funds no longer pass validation
- **THEN** final action validation rejects the purchase
