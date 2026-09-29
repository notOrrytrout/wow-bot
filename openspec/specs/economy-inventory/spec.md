# Inventory and Economy Specification

## Purpose

Define authoritative inventory, loot, vendor, equipment, trade, auction, and mail behavior with explicit asset-transfer safety and stale-transaction protection.

## Requirements

### Requirement: Authoritative inventory state
The system SHALL use current authoritative inventory state for item use, equipment, trade, auction, mail, profession, and bag-space decisions.

#### Scenario: Planned item is no longer present
- **WHEN** an action references an item that current inventory state no longer contains
- **THEN** the action is rejected before mutation

### Requirement: Automatic equipment upgrades use authoritative comparisons
During maintenance, the system SHALL compare usable backpack equipment with the current authoritative equipment state and automatically equip the strongest candidate that improves its destination by at least one percent. It SHALL query missing item metadata before making a comparison and send the equip action through the shared action validation path.

#### Scenario: Backpack contains a usable equipment upgrade
- **WHEN** authoritative item metadata shows that an owned backpack item is usable and improves an equipment slot by at least one percent
- **THEN** maintenance equips the item into the best valid slot
- **AND** it leaves equipment unchanged when no candidate meets the improvement threshold

#### Scenario: Gear metadata is incomplete
- **WHEN** an owned item or currently equipped item lacks item-template metadata
- **THEN** the system queries the missing metadata
- **AND** it does not compare or equip that item until the metadata is authoritative

### Requirement: Equipment repair uses observed durability and repair service
The system SHALL repair equipment only when observed equipped-item durability shows a broken item or a lowest durability below 25 percent. Unknown durability SHALL NOT authorize repair. A repair action SHALL target a currently observed, interactable unit whose entry is listed as repair-capable in trusted world knowledge, and SHALL pass through the shared action validator. The system SHALL not leave an active group to perform routine repair and SHALL use a bounded wait and retry after sending a repair request.

#### Scenario: Equipped item is broken or below the repair threshold
- **WHEN** complete authoritative equipment state shows a broken equipped item or a lowest durability below 25 percent
- **THEN** maintenance may travel to a trusted repair vendor and repair the equipment
- **AND** the repair request uses the observed vendor GUID and does not use guild-bank funds

#### Scenario: Equipment durability is unknown or healthy
- **WHEN** equipment state is incomplete or all observed items are at or above 25 percent durability
- **THEN** the system does not send a repair request

#### Scenario: Group is active
- **WHEN** authoritative group state marks the group as active
- **THEN** routine repair does not start a vendor detour

#### Scenario: Repair result is delayed
- **WHEN** a repair request is sent but equipment durability has not changed
- **THEN** the lane waits for authoritative durability state and applies a bounded retry delay

### Requirement: Quest reward selection compares authoritative equipment
When an authoritative quest reward offer contains multiple item choices, the system SHALL use item metadata and current equipment metadata to select the strongest usable upgrade. It SHALL request missing metadata, wait for a bounded interval, and then select from the server's offered choices using available scores. It SHALL use a deterministic value fallback when no offered item is a proven upgrade.

#### Scenario: One offered reward improves current equipment
- **WHEN** an offered item is usable and improves an equipment slot by at least one percent
- **THEN** the system selects the offered item with the strongest upgrade
- **AND** it sends the server's zero-based reward choice through the shared action validation path

#### Scenario: Reward metadata is delayed
- **WHEN** item metadata needed for reward comparison is not available
- **THEN** the system requests the missing metadata and waits for a bounded interval
- **AND** it selects only from the authoritative server offer after the wait expires

### Requirement: Authoritative loot completion
The system SHALL not treat an attempted loot action as successful until authoritative state confirms the loot outcome.

#### Scenario: Loot attempt receives no completion evidence
- **WHEN** loot is attempted but authoritative state does not confirm completion
- **THEN** the system does not record the target as successfully looted solely from the attempt

#### Scenario: Corpse position is stale after an owned kill
- **WHEN** the server confirms a player or pet kill and the creature still has a stale spawn position
- **THEN** the bot approaches the best current corpse position supported by authoritative state and the kill observation
- **AND** it does not use the stale spawn position when a nearby killer position is available

### Requirement: Grounded vendor transactions
The system SHALL require a current valid vendor/service interaction state before committing a vendor transaction.

#### Scenario: Remembered vendor is not currently interactable
- **WHEN** memory or world knowledge identifies a vendor location but current state does not establish a valid service interaction
- **THEN** navigation may use the memory under policy
- **AND** the transaction itself is not executed

### Requirement: Explicit asset-transfer policy
The system SHALL disable trade, auction, mail, or other asset mutation unless the applicable safety configuration and plan origin authorize it.

#### Scenario: Asset mutation is disabled
- **WHEN** an action would transfer money or items while asset mutation is disabled
- **THEN** validation rejects the action

### Requirement: Trade generation binding
The system SHALL reject stale multi-step trade operations when the observed trade generation or transaction intent no longer matches the action.

#### Scenario: Trade window changes after plan creation
- **WHEN** trade state advances to a new generation before a prepared transfer executes
- **THEN** the stale transfer is rejected

### Requirement: Gift acceptance is distinct from reciprocal trade
The system SHALL require gift-specific conditions before accepting a transfer that is intended to give assets to the bot without reciprocal assets.

#### Scenario: Clear gift path is used
- **WHEN** the bot uses the clear-gift acceptance path
- **THEN** the bot offers no assets
- **AND** the partner's qualifying offer remains unchanged through final validation

### Requirement: Auction listing freshness
The system SHALL bind auction purchases to current query/search generation and SHALL reject stale listing references.

#### Scenario: Auction search changes
- **WHEN** a listing was selected from an older search generation
- **THEN** the listing is not purchased without current validation

### Requirement: Mailbox state is authoritative
The system SHALL use current mailbox state and SHALL not fabricate mail money or attachments from stale observations.

#### Scenario: Mail contents change
- **WHEN** mailbox state changes after planning
- **THEN** any state-changing mail action revalidates the current contents before execution

### Requirement: Loot is a complete deterministic transaction
A semantic loot action SHALL execute through one shared deterministic loot runtime. Opening a loot window alone SHALL NOT count as completing the loot action. For a bot-owned loot target, the runtime SHALL consume the authoritative loot response, take eligible loot slots and money through the corresponding Wrath client messages, release the loot target, and wait for authoritative inventory, loot-release, or entity-state evidence before the scheduler may retry or consider the operation complete.

#### Scenario: Bot loots a corpse containing quest items
- **WHEN** a validated bot loot action opens an authoritative loot window
- **THEN** the shared loot runtime takes the eligible slots and money and releases the target
- **AND** the quest scheduler does not emit another loot-open request every execution tick
- **AND** quest progress is based on packet-derived inventory or quest-state changes rather than the loot attempt itself

#### Scenario: Loot confirmation is delayed or absent
- **WHEN** a loot transaction does not produce authoritative inventory, release, despawn, or equivalent completion evidence within its bounded timeout
- **THEN** the runtime reports the waiting or timeout reason
- **AND** any retry uses the shared bounded retry policy rather than an unbounded periodic spam loop

### Requirement: Bot-owned loot completion SHALL be causally attributable
A bot loot action SHALL only be considered successfully completed from a loot transaction that was initiated by the bot. Player-initiated `CMSG_LOOT` for the same corpse SHALL supersede any pending bot ownership claim, SHALL be represented as player-owned/external loot state, and SHALL NOT advance the bot-owned loot completion generation. A pending bot loot action superseded by player loot SHALL be cancelled as externally resolved rather than credited as bot success.
