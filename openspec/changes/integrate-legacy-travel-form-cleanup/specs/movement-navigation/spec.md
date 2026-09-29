## ADDED Requirements

### Requirement: Travel abilities are cleared safely after arrival

The lane SHALL clear an observed mount or supported travel form after owned movement reaches its destination. It SHALL use typed gameplay commands and SHALL resume a blocked on-foot action only after authoritative state reports that the player is unmounted and the supported travel-form auras are absent. Cleanup retries SHALL be limited to five attempts over no more than five seconds. The lane SHALL cancel a blocked voluntary action when the mission changes, the lane pauses, or death recovery starts.

#### Scenario: Mounted travel reaches its destination

- **GIVEN** owned movement reaches its destination while authoritative player state reports that the player is mounted
- **WHEN** the lane requests mount cancellation
- **THEN** it sends no blocked on-foot action until authoritative state reports that the player is unmounted
- **AND** it stops retrying after the configured bounded window

#### Scenario: A travel form remains active after arrival

- **GIVEN** owned movement reaches its destination while a supported travel-form aura is observed
- **WHEN** the lane sends a typed aura-cancel command
- **THEN** it waits for the authoritative aura update before it resumes any blocked on-foot action

#### Scenario: An on-foot action is selected while the player is mounted

- **GIVEN** the lane selects an action that requires the player to be on foot
- **AND** authoritative state reports a mount or supported travel form
- **WHEN** the lane begins the action
- **THEN** it requests cleanup and holds the action until authoritative on-foot state is confirmed

#### Scenario: Cleanup is preempted

- **WHEN** the lane pauses, the active mission changes, or death recovery starts during cleanup
- **THEN** the lane cancels the pending voluntary action and sends no stale action after cleanup
