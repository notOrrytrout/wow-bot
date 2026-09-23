# Action Validation and Execution Specification

## Purpose

Define typed action authority, origin and stage restrictions, generation checks, final-send validation, serialization, lifecycle, and bounded retry behavior.

## Requirements

### Requirement: Restricted action authority
The system SHALL expose only authorized action capabilities to each plan origin and SHALL independently validate authority even if an action reaches validation through an unexpected path.

#### Scenario: Untrusted dialogue proposes privileged action
- **WHEN** untrusted player dialogue produces an action that requires gameplay, movement, server-command, logout, or asset-transfer authority
- **THEN** validation rejects the action

### Requirement: Typed plan origin
The system SHALL associate every action plan with a typed origin whose authority is explicitly defined.

#### Scenario: System policy proposes an action
- **WHEN** deterministic system policy creates an action
- **THEN** the action receives only the narrow authority assigned to system policy
- **AND** system policy is not treated as unrestricted trust

### Requirement: Activation-stage enforcement
The system SHALL reject actions that are not permitted by the lane's current activation stage.

#### Scenario: Movement action is proposed below movement stage
- **WHEN** an action requires movement authority but the current stage does not allow movement
- **THEN** execution fails with a stage-specific result

### Requirement: Final-send revalidation
The system SHALL revalidate state generation, mission revision, permission revision, authority, and mechanical legality immediately before a state-changing command is transmitted.

#### Scenario: Action becomes stale after planning
- **GIVEN** an action was valid when planned
- **WHEN** relevant state or generation changes before transmission
- **THEN** the command is not sent

### Requirement: Needs-movement is not sendable
The system SHALL not transmit an action whose current legality requires movement preparation to complete first.

#### Scenario: Target is currently out of actionable range
- **WHEN** final validation determines that movement is required before the action can execute
- **THEN** the action is not transmitted
- **AND** movement may be prepared before validation is run again

### Requirement: Incompatible actions are serialized
The system SHALL prevent incompatible movement, combat, loot, gather, fishing, recovery, maintenance, and other state-changing operations from executing concurrently.

#### Scenario: Survival preempts routine work
- **WHEN** immediate survival work conflicts with lower-priority voluntary work
- **THEN** survival work obtains execution priority
- **AND** still-valid lower-priority work may resume only after normal revalidation

### Requirement: Single terminal action result
The system SHALL give each logical action at most one terminal lifecycle result.

#### Scenario: Action completes
- **WHEN** authoritative observation proves an action has completed
- **THEN** one completion result is recorded
- **AND** later observations do not create a second terminal result for that action

### Requirement: Typed bounded retry
The system SHALL base retry behavior on typed failure reason and SHALL bound retries for repeated unchanged failure.

#### Scenario: Permanent policy failure occurs
- **WHEN** an action fails because policy permanently forbids it under current intent
- **THEN** the runtime does not enter an unbounded retry loop

### Requirement: Retry disposition is typed by recovery cause
Retryable action failures SHALL carry a typed retry disposition that distinguishes recovery causes that require different scheduling behavior, including at least route recomputation, positional recovery, interaction retry, stale-state revalidation, retry after authoritative state change, retry after bounded delay, and non-retryable failure.

#### Scenario: Interaction fails because state became stale
- **WHEN** an otherwise retryable interaction fails due to obsolete authoritative evidence
- **THEN** the runtime waits for or requests fresh state rather than consuming a generic immediate retry counter

#### Scenario: Route computation repeatedly fails
- **WHEN** route computation fails under an unchanged navigation cause
- **THEN** its retry budget is tracked independently from interaction retries
- **AND** exhaustion returns control to strategic scheduling
