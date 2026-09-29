## ADDED Requirements

### Requirement: Battleground lifecycle commands use observed state
The system SHALL expose typed battleground status, random queue, invite port, queue cancel, and match leave commands. Final validation SHALL require battleground mission permission and shall allow invite acceptance, queue cancellation, or match leave only when the current authoritative queue state matches the command's battleground type and lifecycle status.

#### Scenario: Invite acceptance is stale or mismatched
- **WHEN** a battleground port command attempts to enter without a current matching invitation
- **THEN** final validation rejects the command

#### Scenario: A match exit is requested outside an observed match
- **WHEN** a battleground leave command does not match an observed active or leaving match
- **THEN** final validation rejects the command

#### Scenario: Non-system planner requests a battleground lifecycle action
- **WHEN** a language-model or group-policy action proposes a battleground lifecycle command
- **THEN** origin validation rejects the command
