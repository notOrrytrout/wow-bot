# Proxy Local Commands Specification

## Purpose

Define proxy-local `.bot` and `.log` command interception, supported chat surfaces, mission/control commands, passthrough of unrelated dot commands, and command authority boundaries.

## Requirements

### Requirement: Supported `.bot` commands are consumed locally
The proxy SHALL consume recognized `.bot` commands locally rather than forwarding them to AzerothCore as ordinary chat or server commands.

#### Scenario: Player sends `.bot status`
- **WHEN** the command arrives through a supported stock-client chat channel
- **THEN** the proxy handles it locally for the matching configured lane
- **AND** the literal command is not forwarded upstream as a server command

### Requirement: Supported mission commands map to typed missions
The proxy SHALL translate recognized mission-setting commands into the corresponding typed supervisor mission request.

#### Scenario: Player sends `.bot quest`
- **THEN** the lane receives a Quest mission request

#### Scenario: Player sends `.bot gather "Copper Vein"`
- **THEN** the lane receives a Gather mission with the supplied resource text

#### Scenario: Player sends `.bot grind "Defias Bandit"`
- **THEN** the lane receives a Grind mission with the supplied creature text

#### Scenario: Player sends party or raid role command
- **THEN** the lane receives the matching Party or Raid mission with the validated role

#### Scenario: Player sends `.bot goal` with text
- **THEN** the lane receives a Goal mission with that text

### Requirement: Control commands act only on the current lane
`.bot on` and `.bot off` SHALL change worker execution/control state only for the account lane from which the command was issued.

#### Scenario: Account A sends `.bot off`
- **THEN** account A is paused according to handoff rules
- **AND** account B is unchanged

### Requirement: `.log` commands are proxy-local
Recognized `.log` commands SHALL control the proxy Action Log capture locally and SHALL NOT be forwarded to AzerothCore.

#### Scenario: Player starts and marks a log
- **WHEN** `.log start`, `.log mark`, `.log status`, or `.log stop` is valid
- **THEN** the proxy performs the requested local logging operation
- **AND** the literal command is not sent upstream

### Requirement: Unrecognized dot commands are forwarded unchanged
The proxy SHALL NOT capture unrelated dot commands merely because they begin with a dot.

#### Scenario: Player sends an AzerothCore GM command
- **WHEN** the text is not a recognized `.bot` or `.log` command
- **THEN** it is forwarded unchanged according to ordinary chat routing

### Requirement: Local commands are accepted from the supported chat families
The proxy SHALL recognize local commands from the supported stock-client chat message types, including the configured ordinary, group, guild, whisper, emote, raid-warning, and battleground chat surfaces.

#### Scenario: Local command arrives in supported group chat
- **THEN** it is eligible for local parsing exactly as a supported say-channel command would be

### Requirement: Unknown accounts do not receive bot-control commands
Transparent unknown-account sessions SHALL NOT gain `.bot` mission or control authority.

#### Scenario: Non-roster account sends `.bot on`
- **THEN** it is not treated as a configured-lane bot-control operation

### Requirement: Local command handling is visible to the operator
Every recognized configured-account `.bot` or `.log` command SHALL emit an operator-visible diagnostic when the proxy consumes it. Control commands SHALL also emit a diagnostic after the ownership/control transition is applied so reception can be distinguished from successful state change.

#### Scenario: Player sends `.bot off`
- **THEN** the supervisor console reports that `.bot off` was received for the matching account
- **AND** the console reports whether manual ownership was committed
- **AND** the same diagnostic is written to the persistent runtime log

#### Scenario: Player sends `.log status`
- **THEN** the supervisor console reports whether Action Log capture is active
- **AND** when active, it reports the capture path
