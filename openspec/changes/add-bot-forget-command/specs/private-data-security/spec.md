## ADDED Requirements

### Requirement: Protect memory database credentials during forget

The memory clear operation SHALL read the database URL from its configured environment variable, SHALL NOT include the URL in configuration or ordinary logs, and SHALL require verified identity TLS for non-loopback database hosts.

#### Scenario: Remote database does not verify server identity
- **WHEN** `.bot forget` uses a non-loopback MySQL host without `VERIFY_IDENTITY`
- **THEN** the operation fails before it sends any deletion query
