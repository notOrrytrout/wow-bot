# Proxy Session Ownership Specification

## Purpose

Define per-account control ownership, transition generations, fencing, attendance state, source permissions, stale-transition rejection, and isolation across configured lanes.

## Requirements

### Requirement: Ownership is maintained per account
The proxy SHALL keep control mode, requested mode, transition phase, player attendance, attended-control state, generation, and movement epoch independently for each configured account lane.

#### Scenario: Several accounts have players attached
- **WHEN** multiple configured accounts are active concurrently
- **THEN** each account has independent ownership state
- **AND** changing control on one account does not change another account

### Requirement: Ownership transitions fence both command sources
While a control transition is in progress, the proxy SHALL deny ordinary gameplay ownership to both player and bot sources until the transition is committed or failed.

#### Scenario: Transition to manual begins
- **WHEN** the lane begins changing from bot to manual control
- **THEN** the ownership generation advances
- **AND** the movement epoch advances
- **AND** neither source is treated as the ordinary gameplay owner until commit

### Requirement: Transition completion is generation-stamped
The proxy SHALL accept completion or failure only for the currently active transition ticket.

#### Scenario: Older transition finishes after newer request
- **GIVEN** transition A was superseded by transition B
- **WHEN** A later reports success or failure
- **THEN** A cannot mutate current ownership state
- **AND** B remains authoritative

### Requirement: Repeated requests have distinct generations
Repeated requests for the same target mode SHALL still create distinct transition generations so stale asynchronous completion cannot be mistaken for the current request.

#### Scenario: Bot mode is requested twice
- **WHEN** a second bot-mode transition supersedes the first
- **THEN** the two requests have different generations
- **AND** completion of the first cannot commit the second

### Requirement: Source permission follows attended-control state
The proxy SHALL permit gameplay sources according to the current attended-control state rather than from a single global manual-mode switch.

#### Scenario: Player is attached with manual off
- **THEN** player gameplay is permitted as the manual owner
- **AND** bot gameplay is not permitted as the owner

#### Scenario: Player is attached with bot assistance on
- **THEN** bot gameplay may be permitted through the authoritative shared session
- **AND** movement may still yield separately when the player moves

#### Scenario: Player is actively moving while bot assistance is on
- **THEN** player movement owns locomotion
- **AND** bot locomotion remains fenced until the idle-resume handoff completes

### Requirement: Reclaim state blocks gameplay until safe
While the lane is reclaiming headless bot ownership after player logout, neither source SHALL send ordinary gameplay as if ownership were already stable.

#### Scenario: Final player connection closes
- **WHEN** reclaim begins
- **THEN** ownership enters a reclaiming state
- **AND** gameplay remains fenced until the upstream/downstream session transition reaches a safe point

### Requirement: Movement fencing advances with ownership generation
A control transition SHALL advance a movement epoch or equivalent fence so queued locomotion from an older owner cannot resume after ownership changes.

#### Scenario: Manual takeover occurs during bot movement
- **WHEN** ownership generation changes
- **THEN** previously queued bot movement belongs to the old movement epoch
- **AND** it cannot continue as current locomotion without a new valid handoff

### Requirement: Bot-control activation uses prepare and commit
A transition to bot ownership SHALL use a prepare/commit protocol with the owning worker. The proxy SHALL fence stale player/bot work first, request worker preparation under the new transition generation, and commit bot ownership only after the current worker confirms it is ready for that generation. Failure or timeout SHALL leave or restore a safe non-bot owner state.

#### Scenario: `.bot on` is requested
- **WHEN** the player requests bot ownership
- **THEN** the proxy advances the ownership and movement fences
- **AND** the owning worker is asked to prepare under the active transition ticket
- **AND** bot ownership is committed only after matching worker readiness is received

#### Scenario: Worker preparation becomes stale
- **GIVEN** bot transition A is awaiting worker preparation
- **WHEN** a newer transition B supersedes A
- **THEN** a later readiness response for A cannot commit bot ownership
