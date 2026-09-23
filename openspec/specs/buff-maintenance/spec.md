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
