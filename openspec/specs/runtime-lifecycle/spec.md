# Runtime Lifecycle Specification

## Purpose

Define startup, bot-lane lifecycle, immediate player takeover, automatic bot-control restoration, pause semantics, and failure isolation for the multi-lane wow-bot runtime.

## Requirements

### Requirement: Validated startup
The system SHALL validate configuration, required runtime data, protected runtime paths, and lane plans before autonomous gameplay begins.

#### Scenario: Startup prerequisites are valid
- **WHEN** configuration and required runtime data are valid
- **THEN** the runtime starts the supervisor, proxy-facing services, and configured bot lanes
- **AND** gameplay remains gated until supervisor control is established

#### Scenario: Mandatory startup prerequisite is invalid
- **WHEN** a mandatory configuration, runtime-data, or security prerequisite is invalid
- **THEN** startup fails with an actionable error
- **AND** the system does not silently continue with unsafe defaults

### Requirement: Headless autonomous lanes default to questing
After a configured headless lane completes authentication, character selection, authoritative world entry, and normal state/generation gates, the runtime SHALL install a Quest mission by default and enable bot ownership without requiring `.bot on` or `.bot quest` from a stock client. An explicit configured/operator mission MAY replace that default.

#### Scenario: Headless character enters the world
- **WHEN** a configured headless character receives authoritative world-entry confirmation
- **THEN** the lane installs a fresh Quest mission
- **AND** bot ownership is requested only after authoritative world entry
- **AND** the execution clock may begin quest work after normal safety/state gates pass

#### Scenario: Headless login has not reached authoritative world entry
- **WHEN** authentication, Warden, character selection, or world login is incomplete
- **THEN** the runtime does not treat the lane as runnable quest automation

### Requirement: Independent bot lanes
The system SHALL isolate bot-lane lifecycle failures so one lane cannot corrupt or restart unrelated lanes.

#### Scenario: One worker fails
- **WHEN** one bot worker terminates with a recoverable failure
- **THEN** recovery is scoped to that lane
- **AND** unrelated lanes continue operating

### Requirement: Immediate authoritative player takeover
The system SHALL allow an authoritative player to take control of a bot lane at any time that the account's world session can be transferred to player control.

#### Scenario: Player takes control while the bot is active
- **GIVEN** automation is active for a lane
- **WHEN** an authoritative player session takes control of that lane's account
- **THEN** bot-authored gameplay execution for that lane stops immediately
- **AND** pending bot actions that are no longer valid for player control are invalidated or suspended
- **AND** the player receives control of the authoritative world session
- **AND** unrelated lanes continue operating

#### Scenario: Player takeover occurs during bot work
- **GIVEN** the bot is moving, fighting, gathering, questing, or performing another autonomous operation
- **WHEN** authoritative player control is established
- **THEN** the runtime does not require the current bot operation to finish before yielding control
- **AND** no new bot-authored gameplay command is sent for that lane while player control remains active

### Requirement: Automatic bot-control restoration
The system SHALL automatically return a lane to bot control when authoritative player control ends, unless another independent pause or safety condition prevents automation.

#### Scenario: Player stops controlling an existing shared session
- **GIVEN** the lane is paused only because authoritative player control is active
- **WHEN** authoritative player control ends and the existing world session remains usable by the bot runtime
- **THEN** the player-control pause is cleared automatically
- **AND** the bot revalidates current state, mission revision, permission revision, and execution authority
- **AND** automation resumes without requiring `.bot on` or another manual resume command

#### Scenario: Player disconnects and the shared world session ends
- **GIVEN** the lane is paused only because authoritative player control is active
- **WHEN** the player's final authoritative connection ends and no reusable world session remains
- **THEN** the player-control pause is cleared automatically
- **AND** the lane is eligible to establish a fresh headless bot session
- **AND** automation resumes after normal login, state synchronization, and generation validation succeed

#### Scenario: Another pause reason remains after player control ends
- **GIVEN** a lane is paused by player control and by an independent operator or safety pause
- **WHEN** player control ends
- **THEN** only the player-control pause is cleared automatically
- **AND** automation remains stopped until the independent pause or safety condition is cleared

#### Scenario: State changed while the player had control
- **GIVEN** the player changed position, target, combat state, inventory, quest state, group state, or another authoritative state while controlling the lane
- **WHEN** bot control is restored
- **THEN** the bot resumes from newly observed authoritative state
- **AND** stale actions, routes, targets, or transactions from before player takeover are not blindly resumed

### Requirement: Explicit local control remains available
The system SHALL keep explicit local bot-control commands available even though normal hand-back from player control is automatic.

#### Scenario: Player explicitly disables automation
- **WHEN** the player uses the supported local command to keep automation disabled
- **THEN** the lane remains paused after ordinary player activity stops
- **AND** automatic hand-back does not override that explicit pause

#### Scenario: Player explicitly enables automation
- **WHEN** the player uses the supported local command to enable automation
- **THEN** automation may resume on the authoritative session after normal state, generation, and safety validation

### Requirement: Independent pause reasons
The system SHALL track independent causes of pause without clearing unrelated pause causes.

#### Scenario: Operator and player pauses overlap
- **GIVEN** a lane is paused by both operator policy and player control
- **WHEN** the operator pause is cleared
- **THEN** the player-control pause remains active

#### Scenario: Player control ends while operator pause remains
- **GIVEN** a lane is paused by both operator policy and player control
- **WHEN** player control ends
- **THEN** the player-control pause is cleared
- **AND** the operator pause remains active

### Requirement: Safe hand-back invalidates stale bot work
The system SHALL prevent bot work prepared before or during player takeover from executing after control returns unless that work is still valid under current authoritative state and current generations.

#### Scenario: Pre-takeover action becomes stale
- **GIVEN** a bot action was prepared before player takeover
- **WHEN** player control changes the world state or a relevant mission or permission generation
- **THEN** the stale action is rejected after hand-back
- **AND** the bot replans from current authoritative state

#### Scenario: Pre-takeover work remains valid
- **GIVEN** strategic work existed before player takeover
- **WHEN** control returns and the work remains valid under current state and generation checks
- **THEN** the runtime may resume or reconstruct that work according to normal task-lifecycle rules
- **AND** it still performs final validation before sending gameplay commands

### Requirement: Safe worker replacement
The system SHALL prevent obsolete worker connections from terminating or mutating a newer replacement worker session.

#### Scenario: Old worker disconnects after replacement
- **WHEN** a replacement worker has become authoritative for a lane and the old worker later disconnects
- **THEN** the replacement worker remains active

### Requirement: Watchdog recovery
The system SHALL distinguish expected takeover, cancellation, protocol failure, I/O failure, and startup/login timeout so recovery policy can act on the correct failure class.

#### Scenario: Worker never reaches login progress
- **WHEN** a worker exceeds the configured login-progress deadline
- **THEN** the lane is eligible for watchdog recovery
- **AND** unrelated lanes are not restarted

#### Scenario: Player takeover disconnects or replaces bot transport
- **WHEN** worker or session transport changes because authoritative player takeover occurred
- **THEN** the event is classified as expected takeover behavior rather than an unrelated fleet failure
- **AND** unrelated lanes are not restarted

### Requirement: Per-lane execution clock
Each configured lane SHALL own a bounded periodic execution clock that advances runnable mission work independently of incidental packet arrival. The clock SHALL skip or coalesce missed ticks rather than accumulating an unbounded backlog, and one lane's slow tick SHALL NOT stall another lane.

#### Scenario: No world packet arrives during runnable work
- **GIVEN** a non-idle mission is runnable
- **WHEN** no relevant network packet arrives during the next execution interval
- **THEN** the lane still receives an execution opportunity from its own clock
- **AND** mission liveness does not depend on unrelated packet traffic

#### Scenario: Lane execution falls behind
- **WHEN** one or more scheduled ticks are missed because the lane is busy
- **THEN** the runtime skips or coalesces stale ticks
- **AND** it does not enqueue an unbounded tick backlog
