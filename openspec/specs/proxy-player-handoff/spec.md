# Proxy Player Handoff Specification

## Purpose

Define immediate player takeover, live-session bot assistance, movement ownership, idle automatic bot resume, explicit `.bot on` and `.bot off`, final-player logout reclaim, and safe headless restoration.

## Requirements

### Requirement: Player login takes control before its world session becomes authoritative
For a configured running bot account, the proxy SHALL pause that account's bot execution before sending the player's independent `CMSG_AUTH_SESSION` upstream.

#### Scenario: Player logs into a running bot account
- **WHEN** the configured player world connection is preparing to authenticate upstream
- **THEN** only that account's worker execution is paused before the player session becomes authoritative
- **AND** the old bot WorldSession may then be replaced normally by AzerothCore

### Requirement: Player takeover does not disconnect the worker
The worker SHALL remain connected to the proxy with its execution gate closed during manual player control so control can later return without reconstructing the worker process.

#### Scenario: Player takes manual control
- **THEN** the worker remains attached to the lane
- **AND** the player uses the authoritative world session
- **AND** unrelated bot lanes continue

### Requirement: Explicit bot assistance reuses the live player session
`.bot on` SHALL enable bot assistance for that lane without disconnecting the player or opening a second authoritative WorldSession.

#### Scenario: Player runs `.bot on`
- **THEN** the worker execution gate is resumed after the required handoff barriers
- **AND** worker actions use the player's existing authoritative upstream session
- **AND** the player remains logged in

### Requirement: Explicit manual mode stops bot execution safely
`.bot off` SHALL pause worker execution, fence queued bot locomotion, and restore player control without requiring reconnect.

#### Scenario: Player runs `.bot off` during bot movement
- **THEN** queued bot locomotion is fenced
- **AND** one authoritative movement stop is sent when a canonical position is available
- **AND** worker execution is paused
- **AND** the stock client remains connected and in control

### Requirement: Mission commands enable bot assistance on the live session
A valid mission-setting `.bot` command SHALL install the requested mission and enable bot execution through the existing authoritative player session.

#### Scenario: Player selects a gather mission
- **WHEN** the player sends a valid local gather mission command
- **THEN** the supervisor receives the mission update for that lane
- **AND** bot assistance is enabled through the same live session after normal validation

### Requirement: Player movement immediately owns locomotion
When bot assistance is enabled and the player supplies physical movement, the proxy SHALL yield locomotion to the player instead of requiring bot work to finish.

#### Scenario: Player moves while bot assistance is active
- **WHEN** qualifying player movement is observed
- **THEN** the movement handoff generation advances
- **AND** autonomous bot movement is fenced
- **AND** player locomotion is forwarded as authoritative movement
- **AND** bot assistance remains eligible to resume later

#### Scenario: Passive client movement feedback does not steal bot locomotion
- **GIVEN** bot locomotion is active and the attached stock client receives bot-originated movement visualization
- **WHEN** the client emits heartbeat, stop, fall-land, or other passive movement-state feedback without a new start/turn/jump/facing intent
- **THEN** that feedback SHALL NOT advance the movement handoff generation
- **AND** it SHALL NOT be forwarded upstream in a way that overwrites the bot-owned movement stream
- **AND** the proxy SHALL continue to treat the current bot movement generation as authoritative

#### Scenario: Explicit physical movement preempts bot locomotion
- **GIVEN** bot locomotion is active
- **WHEN** the client emits an explicit movement-intent opcode such as movement start, strafe start, turn/pitch start, jump, ascend/descend start, or facing/pitch change
- **THEN** the proxy SHALL classify that packet through the canonical player-intent classifier
- **AND** the player SHALL take locomotion immediately
- **AND** the qualifying opcode or equivalent typed reason SHALL be observable in diagnostics

### Requirement: Bot locomotion automatically resumes after player idle
When bot assistance remains enabled after temporary player movement, the proxy SHALL automatically restore bot locomotion after the player-idle grace conditions are satisfied.

#### Scenario: Player stops moving
- **GIVEN** bot assistance is enabled and idle resume is armed
- **WHEN** no newer player movement occurs for the configured idle window and any channel grace has ended
- **THEN** the proxy performs the required upstream barrier and movement stop handling
- **AND** bot execution resumes without requiring another `.bot on`

#### Scenario: Player moves again before idle deadline
- **WHEN** new player movement occurs before automatic resume
- **THEN** the idle deadline is restarted from the newer activity
- **AND** bot locomotion does not reclaim early

#### Scenario: Channel grace lasts beyond movement idle window
- **WHEN** the channel-block deadline is later than the ordinary movement-idle deadline
- **THEN** automatic bot resume waits for the later channel deadline

### Requirement: Explicit manual-off disarms automatic idle resume
An explicit `.bot off` SHALL prevent ordinary player inactivity from automatically re-enabling bot execution.

#### Scenario: Player explicitly disables automation
- **WHEN** `.bot off` is active
- **THEN** idle-resume arming is disabled
- **AND** ordinary inactivity does not override the explicit manual choice

### Requirement: Final player disconnect reclaims the running bot lane automatically
For a configured running bot lane, closing the final player world connection SHALL trigger safe automatic return to headless bot ownership.

#### Scenario: Final player connection closes
- **THEN** the account enters reclaiming state
- **AND** the old bot-side world socket is fenced for recycle while execution remains paused
- **AND** stale player-session state is not used for new bot actions
- **AND** the bot lane performs the required fresh bootstrap or reconnect
- **AND** execution resumes after synchronization and handoff barriers succeed

### Requirement: Overlapping player connections use reference-counted presence
The proxy SHALL retain player ownership while any current player world connection for that account remains active.

#### Scenario: Old socket closes after reconnect created a newer socket
- **GIVEN** two player connections briefly overlap during reconnect
- **WHEN** the older connection closes
- **THEN** the account remains player-present
- **AND** the older socket cannot clear or detach the newer upstream bridge

### Requirement: Configured-but-not-running accounts do not reclaim a nonexistent bot
If a configured account is not associated with a running bot worker, final player logout SHALL NOT fabricate an unattended bot reclaim.

#### Scenario: Player leaves configured non-running account
- **THEN** player presence is released
- **AND** no nonexistent worker is resumed
- **AND** idle-resume state is cleared
