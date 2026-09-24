# Proxy Diagnostics and Action Log Specification

## Purpose

Define proxy lifecycle diagnostics, structured handoff events, local Action Log capture, sensitive-payload redaction, bounded logging behavior, and per-lane troubleshooting expectations.

## Requirements

### Requirement: Proxy lifecycle events are observable
The proxy SHALL emit diagnostics for material authentication, listener, ownership, player-attendance, handoff, reclaim, and lane-recovery transitions.

#### Scenario: Player takes over a configured lane
- **THEN** diagnostics identify the account/lane and transition state without exposing credentials

#### Scenario: Lane upstream transport fails
- **THEN** diagnostics state that recovery is lane-local and include the material failure context

### Requirement: Ownership diagnostics include semantic state
Control diagnostics SHALL expose meaningful state such as manual/bot mode, attendance, attended-control state, transition generation, or equivalent semantic fields needed to distinguish takeover, user movement, idle resume, and reclaim.

#### Scenario: Player temporarily owns movement during bot assistance
- **THEN** diagnostics distinguish temporary user-movement takeover from explicit `.bot off`

### Requirement: Action Log commands control capture locally
The proxy SHALL support local Action Log start, mark, status, and stop behavior through the `.log` command surface.

#### Scenario: User adds a log marker
- **THEN** the marker is added to the active local capture with the supplied label
- **AND** the marker command is not forwarded as AzerothCore chat

### Requirement: Sensitive chat and Warden payloads are redacted from Action Log
Action Log capture SHALL redact or omit protected chat and Warden payload bodies according to the implemented capture policy.

#### Scenario: Captured traffic contains chat
- **THEN** sensitive chat contents are not stored as unrestricted raw payload data

#### Scenario: Captured traffic contains Warden exchange
- **THEN** Warden payload contents remain redacted or excluded according to policy

### Requirement: Authentication secrets are not ordinary Action Log content
Authentication material, credentials, SRP secrets, session keys, and equivalent protected capability data SHALL NOT be emitted into ordinary structured action capture.

#### Scenario: Authentication handshake occurs during capture
- **THEN** protected authentication secrets are not recorded as ordinary action-log payloads

### Requirement: Diagnostics preserve lane isolation
A high-volume or failing lane SHALL NOT require a fleet-wide proxy restart merely to preserve diagnostics.

#### Scenario: One lane repeatedly re-authenticates
- **THEN** diagnostics remain attributable to that lane
- **AND** unrelated listener lifecycle remains independent

### Requirement: Structured runtime diagnostics use bounded per-lane streams
The runtime SHALL write metadata-only JSONL records to per-lane navigation,
movement-heartbeat, transition, network, proxy-session, and proxy-movement
streams under the bot-owned log directory. Worker and proxy records SHALL use
the same writer schema and write rules. Each record SHALL include a record type,
format version, Unix timestamp in milliseconds, per-run per-file sequence, run ID, and
lane ID. Each stream SHALL include a run-start record. A bounded nonblocking
queue SHALL drop diagnostics rather than block gameplay when the writer falls
behind, and it SHALL report dropped-record totals. These records SHALL NOT
include credentials, chat text, Warden contents, or raw packet bodies.

#### Scenario: Supervisor starts workers for two lanes
- **THEN** each lane receives separate files for its four worker streams and
  two proxy streams
- **AND** each stream records a run-start event that identifies the supervisor run
- **AND** records from workers and proxy use the same common fields

#### Scenario: Structured log storage is slow or unavailable
- **THEN** gameplay and proxy work do not wait for disk I/O
- **AND** a full queue drops diagnostic records and reports the drop total
- **AND** failure to open or write one stream does not stop runtime execution

### Requirement: Runtime diagnostics are persisted in bot-owned storage
The supervisor SHALL write normal runtime diagnostics to a persistent file under the bot-owned log directory while also emitting them to the interactive console.

#### Scenario: Supervisor starts normally
- **THEN** startup diagnostics identify the exact runtime-log path and Action Log directory
- **AND** neither path is under the read-only AzerothCore runtime-data directory

### Requirement: Action Log files are created under bot-owned logs
`.log start` SHALL create a per-account Action Log file under the bot-owned log directory. `.log status` and `.log stop` SHALL report the exact active or completed file path.

#### Scenario: User starts Action Log capture
- **THEN** a new per-account JSONL capture is opened under the bot-owned log directory
- **AND** packet records include direction, opcode, length, and a non-secret fingerprint rather than unrestricted protected payload bodies
