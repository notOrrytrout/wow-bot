# Combat Specification

## Purpose

Define deterministic combat legality, offensive authorization, self-defense, spell semantics, resource checks, crowd-control safety, and pet boundaries.

## Requirements

### Requirement: Deterministic mechanical combat
The system SHALL determine routine combat legality, rotation mechanics, resource use, healing mechanics, interrupts, range, cooldown, and continuation through deterministic runtime logic.

#### Scenario: Autonomous LLM is disabled
- **WHEN** deterministic decision mode is active and combat is already mechanically authorized
- **THEN** supported routine combat can continue without requiring an LLM response

### Requirement: Explicit voluntary offensive authority
The system SHALL fail closed for voluntary offensive actions unless current mission, permission, and encounter authority permit the target.

#### Scenario: Unauthorised voluntary target is selected
- **WHEN** a voluntary offensive action targets an entity outside the current allowed encounter set
- **THEN** the offensive action is rejected

### Requirement: Self-defense authority is separate
The system SHALL allow bounded defensive response to entities attacking the player, an owned pet, or an applicable active group member without converting that response into permanent voluntary target authority.

#### Scenario: Unplanned attacker engages the player
- **WHEN** an otherwise unauthorized hostile entity attacks the player
- **THEN** defensive combat may respond to that attacker
- **AND** the attacker does not become permanently authorized for later voluntary pulls

#### Scenario: NPC attacks protected group state without bot pull ownership
- **WHEN** a live NPC/unit authoritatively targets the player, the active controlled mover, or an online active group member
- **THEN** that unit is part of the active defensive encounter even if the bot did not initiate the pull
- **AND** defensive combat preempts voluntary mission movement and maintenance work
- **AND** the response reuses the shared combat selector, spatial validation, movement, facing, and cast paths
- **AND** kill/tap/loot ownership remains a separate concern

#### Scenario: Another player merely targets the bot
- **WHEN** an ordinary player entity targets the bot outside explicit PvP encounter authority
- **THEN** target selection alone does not authorize an automatic attack

### Requirement: Battleground PvP authority is contextual
The system SHALL authorize proactive hostile-player combat under battleground mission authority only when current battleground state proves the applicable battleground context.

#### Scenario: Battleground mission outside battleground
- **WHEN** a battleground mission is configured but current state does not prove the bot is inside the battleground context
- **THEN** battleground-specific proactive player combat is not authorized

### Requirement: Reviewed spell semantics
The system SHALL use reviewed spell identity and authoritative ability metadata for semantic combat decisions and SHALL fail closed when an unknown spell identifier lacks required reviewed semantics.

#### Scenario: Unknown spell has familiar display name
- **WHEN** an unknown spell identifier has a display name similar to a known semantic family
- **THEN** name similarity alone does not grant that semantic meaning

### Requirement: Spell legality before transmission
The system SHALL verify applicable knowledge, cooldown, global cooldown, resources, runes, range, target legality, reagents, equipment, cast state, mission authority, and encounter authority before transmitting a spell action.

#### Scenario: Required resource is missing
- **WHEN** an otherwise selected spell lacks a required authoritative resource or reagent
- **THEN** the spell is not transmitted

### Requirement: Crowd-control preservation
The system SHALL consider breakable crowd control and known immunities before voluntary offensive continuation.

#### Scenario: Voluntary attack would break protected crowd control
- **WHEN** current policy marks the target's crowd control as protected from routine breakage
- **THEN** the voluntary attack is withheld unless a higher-priority explicit rule authorizes it

### Requirement: Pet state does not create encounter authority
The system SHALL treat pet observations as state and SHALL not use a pet's selected target alone to authorize a voluntary encounter.

#### Scenario: Pet targets unauthorized hostile
- **WHEN** an owned pet targets a hostile entity that lacks voluntary encounter authority
- **THEN** the pet target alone does not authorize a new voluntary attack

### Requirement: Caster quest combat uses ranged spell policy before melee
For caster classes with authoritative mana and known offensive spells, quest combat SHALL engage hostile targets through the shared deterministic spell selector instead of defaulting to melee auto-attack. Melee is a last-resort fallback only when the caster is authoritatively out of mana and has no usable equipped wand/ranged fallback.

#### Scenario: Caster has mana and an offensive spell
- **WHEN** a Priest, Mage, or Warlock has authoritative mana greater than zero and a known supported offensive spell
- **THEN** quest combat selects the highest known supported offensive spell for the target
- **AND** it does not issue `CMSG_ATTACKSWING` merely because the objective is a creature

#### Scenario: Caster is out of mana with a wand equipped
- **WHEN** an applicable caster is authoritatively out of mana
- **AND** the equipped ranged slot contains an authoritative wand
- **THEN** combat uses wand Shoot through the shared cast path
- **AND** melee is not selected

#### Scenario: Caster is out of mana without a wand
- **WHEN** an applicable caster is authoritatively out of mana
- **AND** no usable equipped wand/ranged fallback is authoritative
- **THEN** melee auto-attack MAY be used as the final fallback

### Requirement: Spell-cycle combat replans without melee-style timeout
A discrete offensive spell cast SHALL re-enter deterministic combat selection after a bounded spell-cycle interval if the target remains alive. It SHALL NOT wait for the full melee auto-attack timeout as though a single cast were a continuing auto-attack.

#### Scenario: Shadow Bolt does not kill the target
- **WHEN** a Warlock casts an authoritative Shadow Bolt rank and the target remains alive
- **THEN** combat reevaluates after the bounded cast cycle
- **AND** another legal ranged action may be selected without waiting for a 12-second melee timeout

### Requirement: Melee caster fallback requires authoritative ranged-slot absence
For caster fallback, an unobserved ranged equipment slot SHALL NOT be treated as proof that no wand/ranged fallback exists. Melee fallback requires both authoritative OOM state and authoritative equipment state proving no usable wand/ranged fallback.

#### Scenario: Caster is OOM before equipment state is observed
- **WHEN** mana is authoritatively zero
- **AND** ranged equipment authority has not yet been established
- **THEN** combat defers rather than selecting melee
- **AND** melee becomes eligible only after authoritative equipment state proves no usable wand/ranged fallback
