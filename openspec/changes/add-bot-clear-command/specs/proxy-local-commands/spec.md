## ADDED Requirements

### Requirement: Clear the current lane mission locally

The proxy SHALL consume `.bot clear` for a configured account, replace only that account's lane mission with a fresh idle mission, and stop bot control.

#### Scenario: Player clears the current mission
- **WHEN** a configured account sends `.bot clear` on a supported local-command chat family
- **THEN** the proxy replaces that lane's mission with idle
- **AND** bot control stops through the existing ownership transition
- **AND** other lanes remain unchanged
- **AND** the command is not forwarded to the game server
