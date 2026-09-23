# lane-runtime-architecture Specification

## ADDED Requirements

### Requirement: Mutable gameplay state has one lane owner

Each configured bot lane SHALL have one runtime owner for mutable gameplay state. Other tasks MAY provide observations, calculations, or proposals, but SHALL NOT independently mutate that lane's authoritative gameplay state.

#### Scenario: Route computation finishes asynchronously
- **WHEN** a route worker finishes a route calculation
- **THEN** it returns a typed result to the lane owner
- **AND** it does not directly replace movement state

### Requirement: Worker failures are lane-local

Each configured lane SHALL be independently supervised so worker failure, saturation, or replacement for one lane does not reset or stop unrelated configured lanes.

#### Scenario: One worker crashes
- **WHEN** worker A terminates unexpectedly
- **THEN** worker A may be detached or restarted according to policy
- **AND** worker B continues without a fleet-wide ownership or state reset

### Requirement: Configured proxy session exclusively owns upstream writes

For a configured account, one proxy session owner SHALL exclusively write gameplay traffic to the authoritative upstream AzerothCore WorldSession. A worker SHALL NOT maintain a competing authoritative server session for that configured account.

#### Scenario: Worker submits a legal action
- **WHEN** final worker validation produces a sendable action
- **THEN** the worker submits the typed action through the control boundary
- **AND** the configured proxy session performs the upstream write only after its current transport-authority checks pass

### Requirement: Worker and proxy use typed control IPC

Worker/proxy communication SHALL use a typed, versioned control protocol rather than a synthetic downstream WoW authentication and WorldSession between the two wow-bot processes.

#### Scenario: Ownership changes while worker is active
- **WHEN** the proxy commits a new ownership generation
- **THEN** the worker receives a typed ownership/fence update
- **AND** no second WoW login handshake is required between worker and proxy

### Requirement: Tentacli is isolated behind an adapter

Tentacli-specific packet, object, and lifecycle types SHALL be translated through a dedicated adapter boundary before they enter wow-bot semantic state, policy, or execution modules. Tentacli decoded state SHALL NOT by itself be treated as wow-bot authoritative semantic state or action authority.

#### Scenario: Tentacli exposes a current object position
- **WHEN** the adapter observes a position through Tentacli's WotLK object API
- **THEN** it emits the corresponding typed protocol observation
- **AND** the wow-bot reducer decides how that observation changes authoritative state

### Requirement: Authoritative state is reducer-owned

Dynamic gameplay truth SHALL be modified through the lane's authoritative state reducer from parsed protocol/domain observations. Static world knowledge, memory, configuration, and model output SHALL remain distinct evidence classes.

#### Scenario: Static data predicts an NPC location
- **WHEN** static knowledge says an NPC is normally present at a location but no current authoritative observation proves presence
- **THEN** the lane does not assert current entity presence from static knowledge alone

### Requirement: Invalidation domains remain distinct

State revision, mission revision, permission revision, worker generation, ownership generation, movement epoch, and activity generation SHALL remain independently representable when they invalidate different work.

#### Scenario: Player movement takes locomotion ownership
- **WHEN** the player advances the movement epoch while the mission remains valid
- **THEN** stale bot locomotion is fenced
- **AND** the durable mission is not invalidated solely because locomotion ownership changed

### Requirement: Async completion is generation-stamped

Asynchronous work whose result can become stale SHALL carry enough logical identity and generation/revision information for the lane owner to reject stale completion.

#### Scenario: Older route finishes after a newer replan
- **WHEN** a route result belongs to an older movement request or generation
- **THEN** the lane owner discards it
- **AND** the older completion does not replace current route geometry

### Requirement: Sendable actions are created only by final validation

The action API SHALL structurally distinguish preparation or rejection results from an action that may be submitted for transmission. `NeedsMovement` or equivalent preparation results SHALL NOT be accepted by the transport boundary as sendable gameplay.

#### Scenario: Spell requires approach movement
- **WHEN** final worker validation determines the target is outside actionable range
- **THEN** validation returns a non-sendable movement requirement
- **AND** the spell must pass final validation again after movement preparation

### Requirement: Proxy performs a final transport-authority fence

Immediately before a configured upstream write, the proxy SHALL recheck the transport authority relevant to that request, including current worker and ownership generation and any applicable movement fence.

#### Scenario: Player takeover races a worker action
- **GIVEN** the worker validated an action under ownership generation N
- **WHEN** player takeover commits generation N+1 before the upstream write
- **THEN** the proxy rejects the stale worker request
- **AND** no gameplay bytes from generation N are written as current bot authority

### Requirement: One lane activity arbiter serializes incompatible activities

Each lane SHALL use one domain activity arbiter to control incompatible state-changing activities. Preemption safety SHALL use an activity generation or equivalent fence so late completion from preempted work cannot mutate the current activity.

#### Scenario: Survival combat preempts fishing
- **WHEN** survival work preempts the fishing activity
- **THEN** the arbiter advances or replaces the fishing activity authority
- **AND** a late fishing completion cannot act as current activity without new validation

### Requirement: Route planning does not own authoritative movement state

Route computation SHALL return route geometry to the lane movement runtime. It SHALL NOT own authoritative character position or independently replace logical movement identity.

#### Scenario: Same movement is replanned
- **WHEN** the lane requests new geometry for the same logical movement
- **THEN** the movement ID and task ownership remain stable
- **AND** only valid current route geometry is replaced

### Requirement: Pause causes are independent

The lane SHALL represent simultaneous pause or execution-blocking reasons independently so clearing one reason does not implicitly clear another.

#### Scenario: Player control ends while operator pause remains
- **WHEN** the player-control reason is cleared
- **THEN** the operator-pause reason remains active
- **AND** ordinary bot execution does not resume until all required blocking reasons are cleared

### Requirement: IPC delivery semantics match message semantics

Latest-value control state MAY use coalesced delivery. Commands requiring ordering or individual acceptance SHALL use bounded ordered delivery and SHALL NOT be silently dropped on saturation.

#### Scenario: Worker command queue is full
- **WHEN** a command requiring individual delivery cannot be accepted because the bounded queue is saturated
- **THEN** the control path reports the unhealthy condition according to worker lifecycle policy
- **AND** it does not report the command as accepted and then silently discard it

### Requirement: Configured and transparent proxy paths remain separate

Configured-account terminated sessions and unknown-account transparent passthrough SHALL remain separate protocol paths with their existing distinct authentication, inspection, and ownership semantics.

#### Scenario: Unknown account connects through passthrough
- **WHEN** an unknown account is admitted to the transparent route
- **THEN** configured-account lane ownership and worker action injection are not applied to that session
