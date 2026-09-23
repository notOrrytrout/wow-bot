# Movement and Navigation Specification

## Purpose

Define deterministic routing, movement identity, timestamp compatibility, coordinate validity, surface continuity, failure handling, and risk-aware route replacement.

## Requirements

### Requirement: Deterministic movement execution
The system SHALL own route computation, route validation, waypoint progression, movement timing, collision and floor handling, movement packet generation, and route retry policy in deterministic runtime logic.

#### Scenario: Strategic movement objective is accepted
- **WHEN** a permitted strategic layer requests movement to a grounded objective
- **THEN** the movement subsystem determines and executes the mechanical route

### Requirement: Stable logical movement identity
The system SHALL preserve one logical movement action across route waiting, progression, temporary suspension, and replanning for the same objective.

#### Scenario: Same objective requires replan
- **WHEN** route geometry changes but the objective remains the same
- **THEN** the movement action retains its logical identity

### Requirement: Compatible movement timestamps
The system SHALL produce bot-authored movement timestamps that do not regress relative to the authoritative or observed movement clock used by the active world session.

#### Scenario: Player-to-bot control transition occurs
- **WHEN** bot movement resumes after movement timestamps from a player-controlled session have been observed
- **THEN** the bot does not begin an unrelated backward timestamp stream

### Requirement: Finite coordinates
The system SHALL reject non-finite movement coordinates.

#### Scenario: Route contains NaN or infinite coordinate
- **WHEN** a movement target or waypoint contains a non-finite component
- **THEN** movement validation rejects it

### Requirement: Mandatory navigation data fails closed
The system SHALL not invent routes when mandatory navigation or collision data required for safe routing is unavailable.

#### Scenario: Required mmap data is unavailable
- **WHEN** the requested movement operation requires missing mandatory navigation data
- **THEN** route construction fails with a typed navigation result

### Requirement: Floor continuity
The system SHALL reject surface corrections that would incorrectly snap movement between unrelated floors.

#### Scenario: Candidate correction is on another floor
- **WHEN** terrain or collision evidence is outside the allowed floor-continuity band
- **THEN** that evidence does not move the route reference to the other floor

### Requirement: Bounded navigation recovery
The system SHALL bound repeated route replanning and SHALL return control to strategic scheduling when an unchanged navigation failure cannot make progress.

#### Scenario: Same route repeatedly fails
- **WHEN** bounded replans reproduce the same material failure
- **THEN** the movement operation reports failure or suppression
- **AND** the runtime does not spin indefinitely

### Requirement: Material risk improvement for routine replan
The system SHALL avoid oscillating between routes for immaterial risk-score differences.

#### Scenario: Alternative route is only marginally different
- **WHEN** a candidate route offers no material safety improvement and there is no immediate danger
- **THEN** the runtime keeps the current viable route

### Requirement: NeedsMovement creates owned movement work
A final-validation result of `NeedsMovement` SHALL transition the originating work item into an owned movement operation that preserves the original work identity and a typed resume intent. The runtime SHALL NOT treat `NeedsMovement` as a terminal log-only result.

#### Scenario: Interaction is out of range
- **GIVEN** a valid quest interaction is selected
- **WHEN** final validation reports `NeedsMovement`
- **THEN** the movement subsystem receives an owned movement operation for the required interaction range
- **AND** successful arrival resumes or revalidates the original interaction intent

### Requirement: Canonical movement state
Each lane SHALL maintain a canonical server-aligned movement state that reconciles player-authored movement, bot-authored movement, server corrections, teleports, and ownership handoffs. Future route and movement decisions SHALL start from this canonical state rather than from an arbitrary last local prediction.

#### Scenario: AzerothCore corrects a predicted bot position
- **WHEN** an authoritative server movement correction contradicts the local predicted pose
- **THEN** the canonical movement state adopts the server correction
- **AND** subsequent bot or player handoff starts from the corrected pose

### Requirement: Movement progress diagnostics
An active movement operation SHALL emit bounded progress diagnostics that include the movement/work identity and enough information to determine whether progress is occurring, such as remaining distance, remaining route points, current retry state, or deadline. Exactly one terminal movement result SHALL be emitted for each movement operation.

#### Scenario: Movement is active but not completing
- **WHEN** movement remains active across multiple diagnostic intervals
- **THEN** diagnostics show its current progress or lack of progress
- **AND** repeated diagnostics are rate-limited according to observability policy

### Requirement: Ground movement uses explicit vertical authority
For ground locomotion, the worker SHALL derive each emitted movement step from the shared movement controller and SHALL choose vertical authority from the movement mode and route evidence. Unrouted ground movement SHALL use the selected read-only AzerothCore `maps/` terrain height at the next X/Y position. Once a Detour/MMAP route has authorized a walkable corridor, the route/navmesh surface SHALL own Z and floor identity for execution; raw terrain height MAY be used as diagnostic evidence but SHALL NOT overwrite or snap a valid routed step onto another vertical layer. If the authoritative vertical source is unavailable, invalid, or implies an unsafe discontinuity beyond the configured step bound, the runtime SHALL stop/replan or report a bounded waiting reason rather than inventing an airborne or cross-floor ground movement packet.

#### Scenario: Unrouted outdoor ground movement has a different static Z
- **WHEN** an unrouted ground mover travels toward a destination whose stored/static Z differs from local terrain
- **THEN** the next movement packet uses terrain-sampled Z at the next X/Y step
- **AND** the worker does not interpolate vertically toward the destination Z

#### Scenario: Routed movement traverses stacked or interior geometry
- **GIVEN** Detour/MMAP has authorized a connected route through a cave, bridge, building, tunnel, ramp, dungeon, or other multi-level geometry
- **WHEN** raw AzerothCore terrain height at an execution X/Y disagrees with the route's authorized floor
- **THEN** the route/navmesh surface remains the vertical authority
- **AND** the runtime preserves route-surface continuity instead of snapping to the unrelated terrain layer

#### Scenario: Execution step crosses a navmesh polygon boundary
- **WHEN** a routed ground step lies between adjacent authorized route polygons
- **THEN** the controller re-projects the step onto an adjacent authorized route surface when possible
- **AND** if optional surface metadata is unavailable, the already-validated route segment supplies bounded vertical interpolation
- **AND** raw terrain height does not replace the route's vertical layer

### Requirement: Controlled flight reuses the shared movement controller
The shared movement controller SHALL support a distinct flight mode for server-authorized controlled movers. Flight mode SHALL be selected only when authoritative controlled-mover movement flags prove flying, can-fly, or disable-gravity capability. Flight mode MAY advance in three dimensions and SHALL preserve the active controlled mover identity and server-authoritative movement flags.

#### Scenario: Eye of Acherus is the active controlled mover
- **WHEN** server state identifies a controlled mover with authoritative flight-capable movement flags
- **THEN** quest travel reuses the shared movement controller in flight mode
- **AND** normal ground terrain clamping is not applied to that controlled mover

### Requirement: Targeted actions use one shared spatial-precondition pipeline
All targeted gameplay actions SHALL use one canonical deterministic spatial-precondition pipeline before transport. The pipeline SHALL evaluate authoritative mover/target state in this order: range, authoritative line-of-sight recovery evidence, facing, then semantic action dispatch. Questing, combat, gathering, economy, group, PvP, and controlled-unit code SHALL NOT maintain separate equivalent range/facing/LOS mechanics.

#### Scenario: Targeted action is out of range
- **WHEN** any targeted semantic action is selected outside its typed interaction/cast range
- **THEN** the shared spatial layer returns owned movement work with the original action as the resume intent
- **AND** mission-specific code does not construct a separate range-recovery implementation

#### Scenario: Target is in range but not faced
- **WHEN** the authoritative mover is in range but outside the action's facing tolerance
- **THEN** the shared spatial layer emits a typed facing operation using the canonical active mover
- **AND** the original action is revalidated after authoritative facing state updates

### Requirement: Facing is a reusable movement primitive
Facing a target SHALL be implemented through one shared deterministic orientation calculation and one typed movement/facing command. The command SHALL use WotLK `MSG_MOVE_SET_FACING` semantics through the normal movement transport path, SHALL update canonical mover orientation, and SHALL participate in movement-generation fencing and attended-control echo suppression.

#### Scenario: Quest item is used on a nearby target
- **WHEN** the target is already in interaction range but the mover is not facing it
- **THEN** the same shared facing primitive used by combat and interaction is used before the item-use action
- **AND** no quest-specific facing packet is emitted

### Requirement: Authoritative LOS and range failures use shared spatial recovery
The runtime SHALL correlate bot-authored targeted casts with AzerothCore cast-failure results. `SPELL_FAILED_LINE_OF_SIGHT`, `SPELL_FAILED_OUT_OF_RANGE`, and `SPELL_FAILED_TOO_CLOSE` SHALL feed the shared spatial recovery layer. Recovery movement SHALL use the same owned movement controller and canonical mover state as ordinary movement, with bounded deterministic retry/reposition behavior.

#### Scenario: Server rejects cast for line of sight
- **WHEN** AzerothCore returns `SMSG_CAST_FAILED` with `SPELL_FAILED_LINE_OF_SIGHT` for a recent bot-targeted cast
- **THEN** the pending semantic action is not treated as completed
- **AND** the shared spatial layer selects a deterministic reposition candidate
- **AND** movement/floor/flight rules are still enforced by the shared movement controller

#### Scenario: Server corrects local range assumption
- **WHEN** AzerothCore reports out-of-range or too-close for a recent bot-targeted cast
- **THEN** the shared spatial layer moves closer or farther respectively
- **AND** the original semantic action is retried only after the spatial recovery is completed and revalidated
