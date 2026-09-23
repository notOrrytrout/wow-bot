# Missions and Tasks Specification

## Purpose

Define durable user intent, mission revision semantics, scoped task ownership, and bounded strategic work without mixing intent with transient execution details.

## Requirements

### Requirement: Typed mission intent
The system SHALL represent durable bot intent with a typed mission that separates strategic intent from transient mechanical state.

#### Scenario: Mission is installed
- **WHEN** a user or operator installs a supported mission
- **THEN** the mission records semantic intent
- **AND** transient route, target, packet, and retry state remain outside the durable mission definition

### Requirement: Supported mission behaviors
The system SHALL support quest, gather, grind, battleground PvP, party, raid, and free-form goal missions with behavior constrained by each mission's semantic scope.

#### Scenario: Gather mission is active
- **WHEN** a gather mission names a resource
- **THEN** voluntary gathering is limited to resources that satisfy the mission's matching rules

#### Scenario: Grind mission is active
- **WHEN** a grind mission names a creature
- **THEN** voluntary target selection does not broaden the name into a loosely similar creature family

### Requirement: Mission revision invalidation
The system SHALL increment a process-local mission revision when mission intent is replaced and SHALL reject work created under an older mission revision.

#### Scenario: Mission changes during pending work
- **GIVEN** an action was created under mission revision N
- **WHEN** a new mission advances the lane to revision N+1
- **THEN** the old action is rejected before execution

### Requirement: Permission revision invalidation
The system SHALL track permission changes independently from mission changes and SHALL reject work created under an obsolete permission revision.

#### Scenario: Execution permissions change
- **GIVEN** an action was prepared under permission revision P
- **WHEN** stage or authority changes advance the permission revision
- **THEN** the action cannot execute under the new permission state without revalidation

### Requirement: Stable task identity
The system SHALL preserve logical task identity across mechanical movement, combat, loot, and route replanning when those operations still serve the same strategic task.

#### Scenario: Route is replanned
- **WHEN** a task requires a new route to the same objective
- **THEN** route geometry may change
- **AND** the strategic task identity remains unchanged

### Requirement: Bounded strategic work
The system SHALL release or replace strategic work that cannot make progress within bounded failure and retry policy.

#### Scenario: Goal target remains unavailable
- **WHEN** repeated bounded attempts cannot reach or observe the target required by a free-form goal task
- **THEN** that work item is released or failed
- **AND** the runtime can select new strategic work

### Requirement: Non-idle mission execution is liveness-observable
The system SHALL supervise every runnable non-idle mission as an end-to-end execution loop rather than treating mission installation as success by itself. Within a bounded interval after a mission becomes runnable, the lane SHALL produce at least one of: authoritative state progress, a validated action attempt, or a specific waiting reason that names the missing evidence or capability.

#### Scenario: Mission is accepted but no work is scheduled
- **GIVEN** a non-idle mission is installed and the lane is runnable
- **WHEN** no action or authoritative progress occurs within the mission liveness interval
- **THEN** the runtime emits an operator-visible waiting or failure reason
- **AND** the mission is not reported as operationally progressing merely because it was accepted by the command parser

#### Scenario: Quest mission starts with no quest evidence
- **WHEN** quest mode is runnable but no authoritative quest-giver status or active quest work is known
- **THEN** the lane performs the bounded authoritative discovery step required by the quest protocol
- **AND** the discovery attempt is visible in diagnostics

### Requirement: Semantic work identity
The runtime SHALL represent active mission work with a stable semantic work identity distinct from the durable mission and from transient mechanical actions. Work identity SHALL identify the concrete planning purpose, such as acquiring a quest, turning in a quest, traveling to an objective, grinding a target, or gathering a required resource.

#### Scenario: Quest mission acquires a quest
- **GIVEN** the durable mission is Quest
- **WHEN** the scheduler selects an available quest to acquire
- **THEN** it creates a quest-acquisition work item with its own stable work ID
- **AND** movement or interaction actions serving that acquisition do not replace the top-level mission identity

### Requirement: Semantic planning keys
Expensive deterministic planning or LLM planning SHALL be keyed by the subset of authoritative state and mission context that materially affects that planning decision, rather than by every raw state revision.

#### Scenario: Irrelevant packet advances state revision
- **GIVEN** a planning result remains valid for the current semantic objective
- **WHEN** an unrelated authoritative observation advances the global state revision
- **THEN** the planner is not required to recompute solely because the unrelated revision changed
- **AND** correctness-critical action validation still uses the current authoritative revisions before send
