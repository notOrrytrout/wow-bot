## Purpose

Define reliable state transitions and decision boundaries for Collection, Gather, and Fishing modes so the bot remains safe, target-scoped, and capable of making deterministic progress.

## ADDED Requirements

### Requirement: Collection completion SHALL drain safety-critical work before logout

When an enabled collection job reaches its target with logout-on-complete enabled, the system SHALL stop starting new voluntary mission work but MUST continue forced death, combat, recovery, and required loot handling until logout is safe.

#### Scenario: Collection completes during combat

- **WHEN** the collection target becomes complete while the player is combat-engaged
- **THEN** the system MUST continue combat/recovery servicing
- **AND** it MUST NOT send a logout request while combat remains engaged
- **AND** it MUST NOT start new voluntary collection work

#### Scenario: Collection completes with loot pending

- **WHEN** the collection target is complete and a loot action or automatic loot opportunity is still pending
- **THEN** the system MUST service that loot state before logout
- **AND** completion MUST NOT bypass loot timeout/recovery handling

#### Scenario: Collection reaches a safe idle state

- **WHEN** collection is complete, combat/recovery/loot are clear, and execution is idle
- **THEN** the system SHALL send exactly one logout request
- **AND** repeated execution ticks MUST NOT send duplicate logout requests

### Requirement: Fishing activity ownership SHALL persist across asynchronous fishing phases

The system SHALL retain the active Fishing activity lease while the Fishing runtime is active, including `WaitBobber` and `UseBobber`, even when no normal action is pending.

#### Scenario: Cast completes and bobber is pending

- **WHEN** a Fishing cast action completes and the Fishing runtime enters `WaitBobber`
- **THEN** idle voluntary lease cleanup MUST preserve the Fishing lease generation
- **AND** the next fishing tick MUST continue the same fishing lifecycle

#### Scenario: Bobber becomes usable

- **WHEN** the Fishing runtime owns its lease and a valid bobber becomes usable
- **THEN** the system SHALL queue/use the bobber without losing the Fishing lease between phases

#### Scenario: Forced activity preempts Fishing

- **WHEN** Death, Combat, or Recovery preempts an active Fishing lease
- **THEN** the Fishing runtime MUST interrupt the stale fishing cycle
- **AND** it MUST NOT resume using the old lease generation

### Requirement: Fishing SHALL travel to a trusted destination before casting

For a Fishing mission with a trusted location-backed fishing hint, the system SHALL deterministically navigate into the fishing destination envelope before starting the fishing cast cycle.

#### Scenario: Fishing destination is out of range

- **WHEN** a trusted Fishing hint exists and the player is outside its arrival envelope
- **THEN** the deterministic mode scheduler SHALL select fishing-resource navigation
- **AND** Fishing MUST NOT cast at the current location

#### Scenario: Player reaches the Fishing destination envelope

- **WHEN** the player reaches the configured Fishing arrival envelope
- **THEN** the fishing travel action SHALL complete based on position
- **AND** it MUST NOT require a fishing-hole game object to hydrate
- **AND** it MUST NOT directly `GAMEOBJ_USE` the fishing-hole object

#### Scenario: No target-specific Fishing location is known

- **WHEN** a resource is classified as Fishing but no trusted target-specific location hint exists
- **THEN** the system MUST NOT cast blindly at the current position
- **AND** supervisor status SHALL identify the missing Fishing location condition

### Requirement: Fishing resource classification SHALL use authoritative generated item data

The world-knowledge layer SHALL classify item names present in authoritative Fishing loot data as `GatherKind::Fishing` without relying on broad name heuristics.

#### Scenario: Fish item name exists in generated Fishing metadata

- **WHEN** a Gather mission resource matches a generated Fishing item name
- **THEN** `gather_kind_for_name()` SHALL return `GatherKind::Fishing`

#### Scenario: Unrelated item contains fishing-like text

- **WHEN** a resource name is not present in the generated Fishing metadata and does not resolve to a Fishing gather node
- **THEN** the system MUST NOT classify it as Fishing solely because of text such as `fish` or `fishing`

### Requirement: Gather mode SHALL detect profession-rank impossibility before exploration

The system SHALL evaluate raw matching gather hints before eligibility filtering so it can distinguish unavailable skill rank from absence of matching world knowledge.

#### Scenario: All matching Mining or Herbalism hints exceed current rank

- **WHEN** trusted same-map matching hints exist and every candidate requires a profession rank above the player's hydrated rank
- **THEN** the supervisor SHALL report profession-rank insufficiency
- **AND** deterministic mode logic MUST NOT fall through to generic exploration for that work

#### Scenario: At least one matching hint is eligible

- **WHEN** some matching hints require a higher rank but at least one trusted candidate is harvestable at the current rank
- **THEN** the system SHALL continue with an eligible candidate
- **AND** it MUST NOT report the mission as rank-impossible

#### Scenario: Fishing location requires a higher Fishing rank

- **WHEN** a location-backed Fishing hint has a required skill above the player's hydrated Fishing rank
- **THEN** the system SHALL report Fishing-rank insufficiency rather than cast or roam

### Requirement: Collection mode SHALL take precedence over the underlying mission gate

When collection is enabled, model-facing voluntary tool gating SHALL be determined by the collection job before Quest, Gather, Grind, or Goal mission gating is applied.

#### Scenario: Item collection runs while underlying mission is Quest

- **WHEN** collection mode is enabled for item or money collection and the stored mission is Quest
- **THEN** the model-facing tool set SHALL remain scoped to allowed collection work and required logistics
- **AND** generic Quest lifecycle tools MUST NOT be reintroduced by the underlying mission gate

#### Scenario: Collection target requires self-defense

- **WHEN** a creature outside the voluntary collection allowlist attacks the player or party
- **THEN** self-defense combat SHALL remain available
- **AND** collection gating MUST NOT suppress forced safety actions

#### Scenario: Collection is complete

- **WHEN** the collection target is complete
- **THEN** model-facing gating MUST NOT expose new voluntary collection work
- **AND** runtime completion-drain rules SHALL control the remaining safety work and logout

### Requirement: Collection quest choices SHALL be privately scoped to the configured quest

For quest collection, the configured target quest ID SHALL be retained only as private runtime/snapshot state and SHALL filter model-facing quest offers before they are projected into tool choices.

#### Scenario: Giver offers target and unrelated quests

- **WHEN** a quest giver offers the configured collection quest and one or more unrelated collectable quests
- **THEN** model-facing quest choices SHALL include the configured collection quest
- **AND** model-facing quest choices MUST exclude unrelated quests

#### Scenario: Collection quest ID is serialized to model context

- **WHEN** sanitized collection state or its schema is serialized for model-facing context
- **THEN** the private target quest ID MUST NOT appear in the serialized output
- **AND** the private creature allowlist MUST remain hidden

#### Scenario: Executor receives an unrelated quest action

- **WHEN** an unrelated quest action bypasses projection and reaches the executor during quest collection
- **THEN** the existing executor authorization check MUST reject it
