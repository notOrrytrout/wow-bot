# Buff Maintenance Baseline Specification

## Requirements

### Requirement: Buff maintenance is cross-mission deterministic service work
Buff maintenance SHALL run from the per-lane execution clock as opportunistic service work rather than as a replacement mission. Durable Quest, Grind, Gather, Group, Battleground, and Goal intent SHALL remain unchanged while maintenance runs.

#### Scenario: Quest mission is active while a self buff is missing
- **WHEN** the lane is bot-owned, authoritative, and otherwise eligible for maintenance
- **THEN** the maintainer MAY issue one bounded maintenance cast
- **AND** the durable quest mission and quest work identity remain unchanged
- **AND** the same execution-clock tick does not enqueue a second gameplay action after the maintenance cast

### Requirement: Aura state is authoritative
The maintainer SHALL decide whether a buff is present from server aura observations. A successful packet write or local cast attempt SHALL NOT by itself prove that the buff is active.

#### Scenario: Maintenance cast is transmitted
- **WHEN** a maintenance spell is sent upstream
- **THEN** the family remains unsatisfied until an authoritative aura update shows a satisfying aura
- **AND** a bounded retry delay prevents immediate duplicate casts

### Requirement: Spellbook and class inputs are authoritative
Maintenance policy SHALL only select spells present in the authoritative known-spell set for the current character class. Initial spellbook state and newly learned spells SHALL update the same shared capability state.

#### Scenario: Higher rank is learned
- **WHEN** the server reports a newly learned higher rank in a maintained family
- **THEN** future maintenance selection uses the highest known acceptable rank
- **AND** no static class list is treated as proof that the character knows the spell

### Requirement: Semantic buff families satisfy equivalent single and group variants
A maintained family MAY contain semantically equivalent single-target and group variants. Existing same-or-better family auras SHALL satisfy maintenance regardless of which equivalent variant produced them.

#### Scenario: Group variant is already active
- **GIVEN** the desired family is Arcane Intellect
- **AND** an equivalent Arcane Brilliance aura of sufficient strength is authoritative on the target
- **WHEN** maintenance evaluates the family
- **THEN** no redundant single-target Intellect cast is issued

### Requirement: Maintenance policy is declarative and mechanics are shared
Class policy SHALL declare which persistent families are eligible for maintenance. Aura lookup, known-rank selection, target selection, validation, retry, and WotLK cast encoding SHALL reuse shared deterministic functions/components rather than class-specific packet/mechanics implementations.

#### Scenario: Missing persistent family is selected
- **WHEN** class policy identifies a missing maintained family
- **THEN** the maintainer produces typed `MaintainBuff` work
- **AND** final validation uses the shared action pipeline
- **AND** WotLK transmission uses the shared spell-cast message construction

### Requirement: Maintenance does not fight higher-priority control
Maintenance SHALL be suspended while the player is dead, while a controlled mover/vehicle owns movement, while player/bot movement is active, while quest work has a pending authoritative action/turn-in/accept transition, or while the lane is not bot-owned/runnable.

#### Scenario: Controlled quest vehicle becomes active
- **WHEN** authoritative state shows a controlled mover
- **THEN** ordinary player buff maintenance is deferred
- **AND** the maintainer does not cast player buffs through the controlled mover

### Requirement: Party maintenance is bounded and local
Party maintenance MAY target online group members for families explicitly marked party-maintainable, but SHALL only do so when the member is already within normal cast range. Buff maintenance SHALL NOT reroute mission travel solely to reach a party member.

#### Scenario: Party member is out of maintenance range
- **WHEN** an online group member is missing a party-maintainable buff but is outside the bounded maintenance range
- **THEN** no movement work is created solely for that buff
- **AND** normal mission work continues

### Requirement: Retry and failure handling are bounded
Each spell/target maintenance pair SHALL have a bounded retry delay after dispatch or authoritative cast failure. Maintenance SHALL NOT enqueue the same missing buff every execution-clock tick.

#### Scenario: Server rejects a buff cast
- **WHEN** an authoritative cast-failure packet corresponds to a maintenance spell/target
- **THEN** that spell/target enters retry backoff
- **AND** other independent maintenance families remain eligible

### Requirement: Procs and conflicting semantic modes are not maintained as ordinary buffs
Proc auras SHALL be observation-only. Mutually exclusive blessings, presences, stances/forms, weapon imbues, and other policy choices SHALL NOT be automatically rotated merely because they are known spells. They require an explicit typed class/spec policy before maintenance.

#### Scenario: Multiple mutually exclusive buffs are known
- **WHEN** policy has not selected one semantic mode
- **THEN** the generic maintainer does not rotate through them
- **AND** knowledge of the spells alone is insufficient to authorize maintenance

### Requirement: Recovery after resurrection can re-establish persistent buffs
After authoritative resurrection and higher-priority recovery work complete, missing persistent self-buff families SHALL become eligible for normal maintenance again without changing the durable mission.

#### Scenario: Character resurrects with buffs removed
- **WHEN** authoritative life/aura state returns to an eligible living state
- **THEN** the maintainer reevaluates its declarative families
- **AND** missing eligible self buffs are restored through the same shared cast path

### Requirement: Early-level persistent buffs are represented by semantic families
A maintenance family SHALL include lower-level predecessor buffs when they are the character's valid persistent version before a stronger family member is learned. Stronger successors MAY satisfy the same semantic family.

#### Scenario: Low-level Warlock knows Demon Skin but not Demon Armor
- **WHEN** the authoritative spellbook contains Demon Skin and no stronger Warlock armor-family spell
- **AND** no satisfying armor-family aura is authoritative
- **THEN** maintenance selects the highest known Demon Skin rank
- **AND** learning Demon Armor later causes the stronger armor-family member to supersede Demon Skin

### Requirement: Maintenance target filtering is positive and bounded
Party maintenance SHALL only consider an online member after proving the member is within maintenance range and missing the semantic family. An out-of-range or already-satisfied member SHALL be skipped rather than selected.

#### Scenario: Party member already has same-or-better buff
- **WHEN** an online nearby party member already has a satisfying family aura
- **THEN** the maintainer skips that member
- **AND** it does not cast simply because retry backoff is absent

### Requirement: Confirmed missing class pets are restored during maintenance
When authoritative pet state confirms that a class pet is absent or dead, maintenance SHALL use a known, ready class summon or recovery spell before ordinary buff work. An active pet SHALL NOT be replaced. For Warlocks, unknown pet control state MAY trigger a bounded summon probe when a known persistent demon summon is ready. Maintenance SHALL retry a known-control summon no sooner than 15 seconds and an unknown-control probe no sooner than 60 seconds after an attempt.

#### Scenario: Warlock has no active demon
- **WHEN** authoritative pet state has no active demon and the Warlock knows a ready persistent demon summon
- **THEN** maintenance selects a summon by specialization and solo/group context, with Imp as the unknown-specialization fallback
- **AND** the summon passes shared spell readiness checks and uses an implicit self-target cast
- **AND** the summon is queued through the shared maintenance action path before ordinary buffs

#### Scenario: Warlock pet control state is unknown
- **WHEN** pet control state is unknown and a known persistent demon summon is ready
- **THEN** maintenance may issue one summon probe
- **AND** maintenance does not issue another summon probe for at least 60 seconds

#### Scenario: Death Knight has Master of Ghouls but no ghoul
- **WHEN** the active talent state includes Master of Ghouls
- **AND** Raise Dead is known and ready
- **AND** authoritative pet state confirms no active ghoul
- **THEN** maintenance uses the shared summon action to restore the ghoul
- **AND** it does not summon a ghoul when Master of Ghouls is not active

#### Scenario: Mage has Eternal Water but no elemental
- **WHEN** active glyph metadata resolves to Eternal Water
- **AND** Summon Water Elemental is known and ready
- **AND** authoritative pet state confirms no active elemental
- **THEN** maintenance uses the shared summon action to restore the elemental
- **AND** it does not summon a temporary elemental without proof of Eternal Water

### Requirement: Restored pets receive their safe default controls
After the server reports a pet spell bar for a summoned or revived pet, maintenance SHALL set the pet to Defensive and enable autocast for its autocastable spells. It SHALL keep control, crowd-control, and other reviewed manual spells disabled. It SHALL enable pet taunts only when the player is not grouped. Maintenance SHALL use server-reported spell-bar state to decide which settings need correction.

#### Scenario: Summoned pet has default controls disabled
- **WHEN** the server reports an active pet in a non-Defensive reaction state with safe autocast spells disabled
- **THEN** maintenance sets Defensive reaction and enables those autocast spells
- **AND** it does not enable a reviewed manual ability

#### Scenario: Pet taunt autocast depends on group state
- **WHEN** the server reports an autocastable pet taunt
- **THEN** maintenance enables it while solo and disables it while grouped

#### Scenario: Pet follows the bot's authorized combat target
- **WHEN** the bot selects an authorized combat target and has an active pet
- **THEN** the pet receives an attack command for that target
- **AND** an unauthorized target does not receive a pet attack command

### Requirement: Mage food and drink reserves use authoritative inventory
Mage maintenance SHALL keep at least ten food items and ten drink items in the backpack when the inventory templates are known. It SHALL select a known, ready Conjure spell and use the shared maintenance cast path. Refreshments that serve as both food and drink SHALL count toward both reserves.

#### Scenario: Mage has food but lacks drink
- **WHEN** the backpack has fewer than ten drinks and a known ready Conjure Water or Conjure Refreshment spell
- **THEN** maintenance casts that spell on the Mage
- **AND** it checks inventory again after the server updates the backpack

#### Scenario: Inventory item templates are still loading
- **WHEN** a backpack item has no authoritative item template
- **THEN** Mage supply maintenance waits
- **AND** it does not infer a supply count from incomplete inventory data

### Requirement: Warlock stones use known spells and current inventory
Warlock maintenance SHALL create a Healthstone when the backpack has none and create a Soulstone when the backpack has none. It SHALL apply an available Soulstone to self only when authoritative self-aura state does not show Soulstone Resurrection. Casts SHALL use known ready spells and item use SHALL use the shared typed item action.

#### Scenario: Warlock lacks a Healthstone
- **WHEN** the backpack has no item with a Healthstone use effect and a known ready Create Healthstone spell
- **THEN** maintenance casts Create Healthstone on the Warlock
- **AND** it waits for authoritative inventory before creating another

#### Scenario: Warlock has an unused Soulstone
- **WHEN** the backpack contains a Soulstone and self aura state does not show Soulstone Resurrection
- **THEN** maintenance uses that observed item instance on self
- **AND** it checks authoritative aura state before trying again

### Requirement: Shaman weapon imbues use explicit specialization policy
Shaman weapon imbues SHALL use a typed specialization policy and authoritative equipped-item state. The maintainer SHALL apply the highest known ready imbue in the specialization priority to an observed unenchanted weapon. Enhancement MAY maintain Flametongue on the off hand. Missing or unknown temporary-enchant evidence SHALL NOT authorize an application.

#### Scenario: Unenchanted main-hand weapon is observed
- **GIVEN** the character is a Shaman and the active specialization is known
- **AND** the main-hand GUID and temporary-enchant state are authoritative
- **WHEN** the main hand has no temporary enchant
- **THEN** maintenance selects the highest known ready imbue for that specialization
- **AND** the typed item-target command is validated against the same equipped GUID before send

#### Scenario: Weapon enchant state is unknown
- **WHEN** the equipped weapon update does not prove whether a temporary enchant is present
- **THEN** maintenance does not apply an imbue to that weapon

### Requirement: Rogue poison application uses authoritative inventory and weapons
Rogue maintenance SHALL apply the highest ranked matching poison from authoritative backpack metadata to an observed unenchanted weapon. The PvE default SHALL use Instant Poison on main hand and Deadly Poison on off hand. It SHALL not apply a poison when weapon enchant state is unknown or already set.

#### Scenario: Rogue has poison and an unenchanted main-hand weapon
- **WHEN** an authoritative backpack item matches the configured poison family and contains a use spell
- **AND** the main-hand instance is observed without a temporary enchant
- **THEN** maintenance issues a typed item-on-item command for those exact item GUIDs
