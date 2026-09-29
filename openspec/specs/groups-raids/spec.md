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

### Requirement: Group following uses role-specific spacing
When the bot follows an observed online group member, it SHALL use the configured group role to select a safe stop distance. It SHALL calculate the follow point in three dimensions and SHALL not cross map boundaries.

#### Scenario: Tank follows the leader
- **WHEN** a Party or Raid mission uses the Tank role and the observed leader is farther away than the tank stop distance
- **THEN** the bot follows the leader and keeps the configured tank gap

#### Scenario: Healer follows the leader
- **WHEN** a Party or Raid mission uses the Healer role and the observed leader is farther away than the healer stop distance
- **THEN** the bot keeps the healer spacing instead of closing to melee distance

#### Scenario: Group members are separated vertically
- **WHEN** the bot and observed member positions differ in height
- **THEN** the follow point preserves the role's three-dimensional stop distance

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

### Requirement: Role-aware pull authorization
The system SHALL initiate voluntary group combat only against the currently observed group encounter target. A configured tank may initiate that target. Other roles SHALL wait until the target is observed in combat against the player or an online group member. Unknown combat or victim state SHALL NOT authorize a voluntary pull.

#### Scenario: Tank initiates the assigned encounter target
- **WHEN** the tank role observes a live hostile as the current group encounter target
- **THEN** the tank may engage that target

#### Scenario: Damage role waits for the tank's pull
- **WHEN** a non-tank role observes the assigned hostile but the hostile is not visibly in combat against the group
- **THEN** the bot waits and does not start combat

#### Scenario: Non-tank assists a group-engaged target
- **WHEN** a non-tank role observes the assigned hostile in combat against an online group member
- **THEN** the bot may assist that target

#### Scenario: Nearby hostile is not assigned to the group encounter
- **WHEN** an observed hostile is not the current group encounter target
- **THEN** role policy does not authorize a voluntary attack against it
