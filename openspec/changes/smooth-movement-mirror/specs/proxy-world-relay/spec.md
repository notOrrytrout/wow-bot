## ADDED Requirements

### Requirement: Bot movement mirroring is ordered and recovers from skipped updates
The configured player bridge SHALL send only the latest valid bot movement mirror update for the current mirror epoch. It SHALL reject duplicate or stale updates. If intermediate updates are coalesced, it SHALL send the latest absolute movement position and record that recovery.

#### Scenario: Player control changes while a mirror is queued
- **WHEN** explicit player movement or a bot ownership or mission command changes the mirror epoch
- **THEN** queued mirror frames from an older epoch are discarded
- **AND** fresh bot movement can be mirrored only under the current epoch

#### Scenario: Client mirror falls behind
- **WHEN** the bridge receives a mirror sequence with a gap
- **THEN** it sends the latest absolute position instead of replaying stale intermediate positions
- **AND** it records the skipped update count

#### Scenario: Client mirror write fails
- **WHEN** writing a movement mirror frame to the player connection fails
- **THEN** the bridge reports the account, sequence, and write error
- **AND** it closes that world bridge so normal reconnect handling can restore the session
