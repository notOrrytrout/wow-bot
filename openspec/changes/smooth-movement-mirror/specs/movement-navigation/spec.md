## ADDED Requirements

### Requirement: Active movement uses frequent bounded steps
The worker SHALL retain the shared navigation step planner and SHALL advance active movement at 100 ms intervals with each step bounded to a configured maximum distance. The mission-planning cadence SHALL remain independent from movement cadence.

#### Scenario: Bot follows a route
- **WHEN** an owned movement operation is active
- **THEN** the worker computes steps through the shared navigation controller at the movement cadence
- **AND** it retains route validation, floor continuity, and movement progress checks

#### Scenario: Movement pose is invalid
- **WHEN** the current position or orientation is non-finite
- **THEN** the proxy rejects the movement command before it encodes or sends a movement packet
