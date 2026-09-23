# bot-lifecycle-architecture Specification

## ADDED Requirements

### Requirement: Logical movement ownership is stable across physical phases

The system SHALL treat movement as active logical work in `AwaitingRoute`, `Moving`, `Suspended`, and tick-owned states. Server control SHALL NOT remove its action ID, task ID, destination, deadline, or pending route.

#### Scenario: Server spline suspends quest movement

- **WHEN** server spline control suspends quest movement
- **THEN** movement remains active and quest maintenance does not release or restart it

#### Scenario: Route planning is pending

- **WHEN** a same-movement route request is pending
- **THEN** the movement and owning quest task remain active

### Requirement: Quest travel retains semantic action identity

The system SHALL preserve action and task identity through replans until stand-off distance or terminal closure. Replans SHALL replace waypoints only.

#### Scenario: Replan occurs during approach

- **WHEN** route recalculation is required
- **THEN** the current authoritative position is used and the semantic action is retained

### Requirement: Action lifecycle uses existing terminal statuses

The system SHALL centralize lifecycle handling and suppress duplicate events. Terminal statuses SHALL be `Completed`, `Failed`, `TimedOut`, or `Interrupted`.

#### Scenario: Action reaches terminal state

- **WHEN** an action closes
- **THEN** exactly one existing terminal status is emitted

### Requirement: Retry and diagnostics are bounded

The system SHALL use stable retry scope and release conditions, and SHALL rate-limit identical diagnostics.

#### Scenario: Rejected action repeats

- **WHEN** the same action is rejected without a release condition
- **THEN** retry is suppressed and identical diagnostics are bounded

### Requirement: Forced activity preserves recoverable work

Temporary preemption SHALL preserve resumable task ownership; explicit terminal replacement SHALL drain it.

#### Scenario: Combat preempts travel

- **WHEN** combat temporarily preempts travel
- **THEN** resumable travel ownership is preserved for later release

### Requirement: Fleet durability remains strict by default

Strict timeline durability SHALL remain available and terminal records SHALL be durable before acceptance.

#### Scenario: Terminal record is accepted

- **WHEN** a terminal record is accepted
- **THEN** it is durable before acceptance completes

### Requirement: Fleet scheduling remains bounded

Provider inflight limits and worker admission bounds SHALL remain enforced; event-driven scheduling is later work after lifecycle stability.

#### Scenario: Many bots await deadlines

- **WHEN** many bots have no ready work
- **THEN** admission and provider inflight limits remain bounded
