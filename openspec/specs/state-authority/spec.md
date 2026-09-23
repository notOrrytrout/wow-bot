# Authoritative State Specification

## Purpose

Define packet-derived state authority, snapshot projection rules, entity lifetime behavior, and stale-state rejection for all gameplay decisions.

## Requirements

### Requirement: Packet-derived authority
The system SHALL derive authoritative dynamic gameplay state from parsed protocol observations and domain-state updates rather than from model assumptions.

#### Scenario: Packet updates entity state
- **WHEN** an authoritative protocol event changes a known entity
- **THEN** domain state is updated before new action validation uses the changed state

### Requirement: Snapshot projection
The system SHALL treat snapshots as projections of authoritative domain state and SHALL not use a snapshot as an independent source of contradictory invariants.

#### Scenario: Snapshot and authoritative state revision differ
- **WHEN** an action references a snapshot revision that is no longer current
- **THEN** final validation rejects or refreshes the action according to policy

### Requirement: Entity destruction removes dependent capabilities
The system SHALL remove state and capabilities that depend on an entity when authoritative state proves that entity no longer exists.

#### Scenario: Service entity is destroyed
- **WHEN** the authoritative stream removes an entity that provided a vendor or interaction capability
- **THEN** that entity is no longer valid for new transactions

### Requirement: Authoritative quest membership
The system SHALL establish active quest membership only from authoritative quest state.

#### Scenario: Static hint references a quest
- **WHEN** world knowledge references a quest but the authoritative quest journal does not show it as active
- **THEN** the system does not treat that quest as active

### Requirement: Desynchronization fails closed
The system SHALL reject state-changing actions when the authoritative state stream is known to be desynchronized for the information required by the action.

#### Scenario: Required state is desynchronized
- **WHEN** final validation detects known state desynchronization that affects target or legality checks
- **THEN** the action fails with a typed desynchronization result
