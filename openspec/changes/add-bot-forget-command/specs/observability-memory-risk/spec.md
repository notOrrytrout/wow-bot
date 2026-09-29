## ADDED Requirements

### Requirement: Clear one bot's persistent memory atomically

The system SHALL delete the configured bot's rows from all supported legacy SQL memory tables in one transaction using a bound bot identity. It SHALL NOT report success when the backend is unavailable or the transaction fails.

#### Scenario: Memory clear commits
- **WHEN** the configured database backend is available and all bot-scoped deletes succeed
- **THEN** the transaction commits the deletion for that bot ID
- **AND** rows for other bot IDs remain unchanged

#### Scenario: Memory clear cannot reach the backend
- **WHEN** the configured database environment variable is missing or the SQL operation fails
- **THEN** the operation returns an error
- **AND** the bot does not report that memory was cleared
