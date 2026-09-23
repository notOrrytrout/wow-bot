# Proxy Observation Specification

## Purpose

Define passive world observation, server-to-worker projection, movement projection to connected stock clients, configured versus transparent account boundaries, and the rule that observation never creates ownership authority by itself.

## Requirements

### Requirement: Server state can be observed without becoming command authority
The proxy MAY inspect supported world traffic for state synchronization, diagnostics, handoff, movement, Warden, or worker projection, but observation SHALL NOT by itself authorize player or bot gameplay commands.

#### Scenario: Proxy observes a server update
- **THEN** the update may inform worker/state projection
- **AND** ownership permission still comes from the lane ownership state

### Requirement: Authoritative server movement is projected to an attached player client
When autonomous bot movement changes the character on a player-attached shared session, the proxy SHALL project the authoritative movement needed for the connected stock client to observe the character motion.

#### Scenario: Bot moves while bot assistance is enabled
- **GIVEN** a stock client remains attached
- **WHEN** the bot owns movement and AzerothCore confirms or broadcasts authoritative movement
- **THEN** the stock client receives the applicable movement update

### Requirement: Server corrections remain authoritative
Server-authoritative movement corrections SHALL take precedence over locally predicted locomotion and SHALL update the movement baseline used for later handoff.

#### Scenario: AzerothCore corrects position
- **WHEN** a server correction is received
- **THEN** the proxy uses the corrected state as the authoritative movement basis
- **AND** later player/bot handoff does not continue from a contradicted stale pose

### Requirement: Player activity used for handoff is scoped to the configured character
Player movement or presence signals SHALL affect idle-resume or handoff only when they belong to the configured player character/session for that lane.

#### Scenario: Character identity does not match configured lane
- **WHEN** observed activity cannot be attributed to the configured character
- **THEN** it does not arm automatic bot resume for that lane

### Requirement: Unknown accounts remain outside configured observation/control coupling
Transparent sessions MAY be relayed and minimally inspected for routing, but SHALL NOT be attached to configured worker state, configured supervisor control, or configured ownership transitions.

#### Scenario: Transparent account sends world traffic
- **THEN** no configured bot worker receives ownership of that account from observation alone

### Requirement: Supported gameplay observations reach the owning worker
For configured accounts, supported AzerothCore server packets that are required by an active gameplay policy SHALL be projected to the owning worker as typed protocol observations while the original packet continues to the attached stock client.

#### Scenario: Quest-giver status packet is received
- **WHEN** the configured session receives `SMSG_QUESTGIVER_STATUS`, `SMSG_QUESTGIVER_STATUS_MULTIPLE`, or `SMSG_QUESTGIVER_QUEST_LIST`
- **THEN** the proxy emits the corresponding typed quest observation to the owning worker
- **AND** the same authoritative server packet remains available to the stock client

### Requirement: World packets are classified before projection
Configured-session server frames SHALL be classified by gameplay relevance before worker projection. Classification SHALL distinguish at least object/world state, quest state, movement state, combat state, inventory/economy state, group state, and traffic that is irrelevant to authoritative bot state.

#### Scenario: Irrelevant server packet is received
- **WHEN** a configured session receives a packet that cannot affect supported authoritative gameplay state
- **THEN** the packet continues to the stock client as required
- **AND** the runtime does not trigger unrelated reducers or planners solely because the packet existed

#### Scenario: Object update packet is received
- **WHEN** the packet can change visible objects or their authoritative fields
- **THEN** it is routed through the object-state projection path
- **AND** only material state changes advance the corresponding authoritative observations
