# Groups and Raids Specification

## Purpose

Define party and raid cooperation from observed group state, including roles, formation, encounter response, recovery, and safe handling of incomplete information.

## Requirements

### Requirement: Observed group-state authority
The system SHALL derive party and raid behavior from observed group membership, role, leader, target, health, and encounter state rather than inventing invisible members or threat state.

#### Scenario: Member is not observed in available group state
- **WHEN** current group state cannot establish a member or role fact required for an action
- **THEN** the system does not invent that fact to authorize the action

### Requirement: Configured role behavior
The system SHALL support role-aware party and raid behavior using the configured role and current observed state.

#### Scenario: Bot is configured as healer
- **WHEN** a party or raid mission is active with healer role
- **THEN** group behavior prioritizes healer-appropriate responsibilities subject to current mechanical legality

### Requirement: Group lifecycle states
The system SHALL support forming, following, engaging, recovering, and regrouping behavior as needed by current group state.

#### Scenario: Encounter ends with group members separated
- **WHEN** combat ends and formation is materially broken
- **THEN** the group runtime may enter regrouping behavior before new voluntary engagement

### Requirement: Group work respects self-defense and encounter authority
The system SHALL apply the same combat authorization and self-defense boundaries to group missions unless an explicit group-specific authority rule applies.

#### Scenario: Group member is attacked
- **WHEN** an applicable active group member is attacked by a hostile entity
- **THEN** defensive combat may respond according to self-defense policy
