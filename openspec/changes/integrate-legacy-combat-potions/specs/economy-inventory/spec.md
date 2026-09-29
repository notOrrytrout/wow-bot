## MODIFIED Requirements

### Requirement: Vendor offers are authoritative and purchases are bounded
The system SHALL derive vendor offers from the server's current inventory response. A purchase SHALL match the observed vendor, slot, item, stock, lot size, and price. Automatic Rogue poison restocking SHALL use only a nearby cataloged seller, buy one lot per attempt, require authoritative item metadata and class/level eligibility, and preserve the configured maintenance money reserve. Food, drink, and bandage restocking SHALL be opportunistic: the system SHALL buy only from an already nearby trusted vendor, use authoritative backpack and item-template data, buy at most one server-defined lot, and preserve at least 1,000 copper. It SHALL wait for an authoritative inventory count change before another recovery-supply purchase, with a bounded timeout if inventory does not update. Unknown item metadata, unsupported recovery-item classification, insufficient stock, extended currency costs, and class- or level-ineligible items SHALL prevent purchase. Recovery supplies SHALL NOT cause vendor travel. Health and mana potions SHALL use reviewed item-name cues and target three units each under the same nearby-vendor, exact-offer, eligibility, stock, one-lot, and reserve rules.

#### Scenario: Low potion supply beside a trusted vendor
- **GIVEN** authoritative backpack item templates show fewer than three health or mana potions
- **AND** a trusted seller is already within interaction range
- **WHEN** its current offer data contains a matching fixed-price potion with sufficient stock and eligible class and level metadata
- **THEN** maintenance buys at most one server-defined lot
- **AND** it preserves at least 1,000 copper
- **AND** it waits for authoritative inventory change or a bounded timeout before another purchase
- **AND** it does not travel to find a vendor

#### Scenario: Potion offer evidence is unsafe or incomplete
- **WHEN** item metadata is unknown, a potion name cue does not match, offer stock is insufficient, the offer has an extended cost, the class or level is ineligible, or the purchase would break the 1,000-copper reserve
- **THEN** maintenance does not buy the offer
