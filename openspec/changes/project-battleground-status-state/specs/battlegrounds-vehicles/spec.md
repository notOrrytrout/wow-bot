## ADDED Requirements

### Requirement: Battleground queue state is typed and scoped to the current world
The system SHALL keep each observed battleground queue slot as typed state, preserve unknown server status values, and clear queue state when the player changes world or leaves the world.

#### Scenario: Server reports an unknown queue status
- **WHEN** a battleground queue status has an unrecognized numeric value
- **THEN** state retains that value as unknown and does not treat it as an empty slot

#### Scenario: Player changes world
- **WHEN** the reducer receives an authoritative world change or leave-world observation
- **THEN** it clears all battleground queue slots from the previous world
