# Supervisor and Control Surface Specification

## Purpose

Define operator control, worker command transport, local player commands, lane-scoped control, and safe handling of high-frequency replaceable control state.

## Requirements

### Requirement: Lane-scoped operator control
The system SHALL expose operator controls for inspecting bots and applying mission, pause, resume, stage, replan, invite, and memory operations to the intended lane or explicit set of lanes.

#### Scenario: One bot is paused
- **WHEN** an operator pauses one bot lane
- **THEN** only the selected lane receives that pause reason

### Requirement: Bounded worker command transport
The system SHALL prevent high-frequency replaceable control state from causing unbounded supervisor memory growth.

#### Scenario: Worker is slow
- **WHEN** replaceable control updates arrive faster than a worker can process them
- **THEN** the supervisor coalesces or bounds those updates according to transport policy

### Requirement: Local bot commands are consumed locally
The system SHALL consume recognized local bot-control commands in the local control layer rather than forwarding them as ordinary server commands.

#### Scenario: User sends .bot off
- **WHEN** a recognized local command requests automation off for the active lane
- **THEN** the lane receives the local pause/control change
- **AND** the literal command is not forwarded as an unrelated server command

### Requirement: Unrecognized dot commands preserve forwarding behavior
The system SHALL preserve forwarding for dot commands that are not recognized as local control commands, subject to normal server-command and chat policy.

#### Scenario: User sends unrelated dot command
- **WHEN** the command is not part of the local bot-control namespace
- **THEN** the local control parser does not reinterpret it as a bot-control action

### Requirement: Literal chat remains literal
The system SHALL not reinterpret ordinary chat text as a privileged server command solely because the text begins with command-like syntax.

#### Scenario: Chat action contains leading dot
- **WHEN** a normal chat action sends text beginning with a dot
- **THEN** it remains chat unless it entered an explicit authorized server-command path

### Requirement: Local action-log controls remain local
The system SHALL consume recognized local action-log control commands without forwarding them to the game server.

#### Scenario: User starts or marks local action logging
- **WHEN** the user invokes a recognized local logging command such as start, mark, status, or stop
- **THEN** the local logging state is updated according to policy
- **AND** the command is not forwarded as an AzerothCore command

