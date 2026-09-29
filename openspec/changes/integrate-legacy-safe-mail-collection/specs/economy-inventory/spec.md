## ADDED Requirements

### Requirement: Automatic mail collection is local and non-COD only
The system SHALL collect attached money and attachments only from current authoritative mail that explicitly has zero COD. It SHALL collect only at exactly one currently observed trusted mailbox within five yards, send one money or attachment action at a time, require at least one free backpack slot for attachments, bind each action to the current mailbox generation, and wait for authoritative mailbox state or a bounded retry timeout before another mail action. Unknown COD state SHALL prevent collection. Mail collection SHALL NOT cause travel or change the active mission intent.

#### Scenario: Safe mail contains money
- **WHEN** authoritative mailbox state has a non-COD mail with attached money and one trusted mailbox is within range
- **THEN** the lane takes the money from that mail
- **AND** it waits for an authoritative mailbox update before another mail action

#### Scenario: Safe mail contains an attachment
- **WHEN** authoritative state identifies an attachment on a zero-COD mail and at least one backpack slot is free
- **THEN** the lane takes that identified attachment
- **AND** it waits for an authoritative mailbox update before another mail action

#### Scenario: Attachment has no backpack space
- **WHEN** authoritative state shows zero free backpack slots
- **THEN** the lane does not take mail attachments

#### Scenario: Mail COD or mailbox evidence is unsafe
- **WHEN** COD is unknown or nonzero, the mail generation is stale, the mailbox is untrusted or out of range, or more than one trusted mailbox is in range
- **THEN** the mail action is rejected or not selected

#### Scenario: Mail response is delayed
- **WHEN** the mailbox generation does not change after a mail action or list request
- **THEN** the lane waits for a bounded timeout and applies a retry delay
- **AND** it does not repeat the action each execution tick
