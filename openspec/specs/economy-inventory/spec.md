# Inventory and Economy Specification

## Purpose

Define authoritative inventory, loot, vendor, equipment, trade, auction, and mail behavior with explicit asset-transfer safety and stale-transaction protection.

## Requirements

### Requirement: Authoritative inventory state
The system SHALL use current authoritative inventory state for item use, equipment, trade, auction, mail, profession, and bag-space decisions.

#### Scenario: Planned item is no longer present
- **WHEN** an action references an item that current inventory state no longer contains
- **THEN** the action is rejected before mutation

### Requirement: Authoritative loot completion
The system SHALL not treat an attempted loot action as successful until authoritative state confirms the loot outcome.

#### Scenario: Loot attempt receives no completion evidence
- **WHEN** loot is attempted but authoritative state does not confirm completion
- **THEN** the system does not record the target as successfully looted solely from the attempt

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
