## ADDED Requirements

### Requirement: Forget persistent memory for the configured bot

The proxy SHALL consume `.bot forget` only from a configured account with an enabled worker and SHALL require the protected debug setting before it can clear persistent memory.

#### Scenario: Authorized bot forgets its memory
- **WHEN** a configured account with an enabled worker sends `.bot forget` and `debug.enabled` is true
- **THEN** only persistent memory rows matching that roster bot ID are cleared
- **AND** the command is not forwarded to the game server

#### Scenario: Forget is not authorized
- **WHEN** a transparent session sends `.bot forget`, the account has no enabled worker, or debug mode is disabled
- **THEN** no database deletion is attempted
- **AND** the command fails with a clear local response
