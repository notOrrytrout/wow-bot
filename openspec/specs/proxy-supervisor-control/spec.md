# Proxy Supervisor Control Specification

## Purpose

Define the authenticated proxy-to-supervisor control channel used for per-lane pause, resume, mission updates, status, and handoff coordination, including failure isolation and absence handling.

## Requirements

### Requirement: Supervisor commands are lane-scoped
The proxy SHALL associate supervisor control requests with the configured bot ID/account lane that originated them.

#### Scenario: Player changes one lane mission
- **WHEN** a valid local mission command is issued on account A
- **THEN** the supervisor request targets account A's bot ID
- **AND** unrelated lanes are not modified

### Requirement: Proxy control uses authenticated supervisor access
When supervisor control is configured, proxy requests SHALL use the configured supervisor endpoint and authentication token rather than an unauthenticated local assumption.

#### Scenario: Proxy creates a supervisor command client
- **THEN** the client is bound to the configured endpoint/token
- **AND** the lane-specific client carries the matching bot ID

### Requirement: Handoff pauses execution before player authority
For a running configured lane, the proxy SHALL successfully request or establish the required execution pause before allowing player-session takeover to proceed as authoritative gameplay.

#### Scenario: Player takeover begins
- **THEN** the proxy requests a lane-local worker pause before upstream player authentication reaches the authoritative point

### Requirement: Resume occurs only after handoff barriers
The proxy SHALL request worker resume only after ownership, movement, and upstream-session barriers required by the applicable handoff have completed.

#### Scenario: Bot resumes after player idle
- **THEN** required upstream synchronization and movement-stop work occurs before supervisor execution resumes

#### Scenario: Bot resumes after final player detach
- **THEN** stale player-session transport is fenced before worker execution resumes

### Requirement: Missing supervisor prevents unsafe reclaim
If a reclaim path requires supervisor coordination and that coordinator is unavailable, the proxy SHALL NOT pretend that the worker resumed successfully.

#### Scenario: Final detach requires reclaim but supervisor is absent
- **THEN** the reclaim is blocked or reported according to lane policy
- **AND** the proxy does not mark unsafe gameplay execution as active

### Requirement: Supervisor errors are explicit
Failed pause, resume, mission, or status operations SHALL produce explicit diagnostic failure instead of silently reporting success.

#### Scenario: Supervisor resume request fails
- **THEN** the proxy logs or returns the failure
- **AND** does not claim that automation has resumed
