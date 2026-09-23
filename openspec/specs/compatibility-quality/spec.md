# Compatibility and Quality Specification

## Purpose

Define cross-cutting behavior-preservation, deterministic operation, concurrency safety, error clarity, and validation expectations for refactors such as deduplication.

## Requirements

### Requirement: Behavior-preserving refactors
The system SHALL preserve externally observable behavior when implementation duplication is consolidated without an intentional behavior change.

#### Scenario: Duplicate helper becomes shared helper
- **WHEN** two equivalent internal implementations are replaced by one shared implementation
- **THEN** former call sites retain equivalent accepted inputs, rejected inputs, outputs, units, and edge-case behavior

### Requirement: Public and framework boundaries remain compatible
The system SHALL preserve required public, trait, serialization, configuration, test, and platform entry points during behavior-preserving refactors.

#### Scenario: Shared implementation crosses a boundary
- **WHEN** a public or framework-required entry point can delegate to a shared helper
- **THEN** the required entry point remains available unless a separate specification explicitly changes that contract

### Requirement: Deterministic no-LLM operation
The system SHALL support its implemented deterministic gameplay and safety behavior when autonomous model decisions are disabled and player dialogue is disabled.

#### Scenario: No model provider is configured
- **WHEN** model-dependent modes are disabled
- **THEN** supported deterministic combat, movement, state handling, survival, and maintenance do not fail solely because no model provider is available

### Requirement: No synchronous lock across asynchronous suspension
The system SHALL avoid holding synchronous mutual-exclusion guards across asynchronous suspension points.

#### Scenario: Async operation needs shared state and I/O
- **WHEN** an operation must access synchronized state and then await external work
- **THEN** it releases the synchronous guard before the await boundary

### Requirement: Typed operational errors
The system SHALL preserve enough error classification to distinguish failures that require different recovery or safety responses.

#### Scenario: Navigation and provider failures differ
- **WHEN** one action fails due to navigation and another fails due to model-provider transport
- **THEN** the runtime exposes distinguishable failure categories rather than reporting both as success or one generic completed state

### Requirement: Regression validation
The system SHALL maintain regression coverage for mission invalidation, permission invalidation, state staleness, combat authority, movement continuity, player takeover, worker replacement, asset safety, provider bounds, and private filesystem behavior.

#### Scenario: Behavior-changing regression is introduced
- **WHEN** a change violates one of the covered cross-boundary contracts
- **THEN** the corresponding validation or test is expected to fail before release

### Requirement: Shared time semantics
The system SHALL preserve the units and timestamp meaning of consolidated time helpers.

#### Scenario: Millisecond timestamp helper is deduplicated
- **WHEN** multiple equivalent current-time helpers are replaced by shared behavior
- **THEN** callers that previously received milliseconds continue to receive milliseconds
- **AND** persisted wall-clock timestamps remain distinct from elapsed-time measurements

### Requirement: Shared distance semantics
The system SHALL preserve dimensionality and invalid-position handling when distance and position helpers are consolidated.

#### Scenario: Three-dimensional distance is calculated
- **WHEN** two finite world positions differ on any spatial axis
- **THEN** a 3D distance contract continues to account for all three axes

#### Scenario: Position is non-finite
- **WHEN** a position contains NaN or infinity
- **THEN** position-validity checks reject it according to the existing contract

### Requirement: Unicode-safe text truncation
The system SHALL preserve valid UTF-8 when shared text truncation is applied.

#### Scenario: Truncation boundary intersects multibyte text
- **WHEN** text is shortened near a multibyte character boundary
- **THEN** the result remains valid UTF-8

### Requirement: Percentage edge-case compatibility
The system SHALL preserve zero-denominator, clamping, and numeric behavior when percentage helpers are consolidated.

#### Scenario: Percentage denominator is zero
- **WHEN** a caller computes a percentage with a zero denominator
- **THEN** the shared behavior matches the pre-refactor zero-denominator contract

### Requirement: Orientation normalization compatibility
The system SHALL preserve the canonical orientation range expected by parsed state and movement behavior.

#### Scenario: Equivalent angles are normalized
- **WHEN** parsed-state and movement code normalize equivalent out-of-range angles
- **THEN** both receive the same canonical orientation convention

### Requirement: Mission runtime reset compatibility
The system SHALL clear transient state from the previous mission when mission-specific runtime state is reset.

#### Scenario: New mission replaces old mission
- **WHEN** a mission runtime is reset for a new mission revision
- **THEN** stale targets, routes, operation leases, and other transient state from the prior mission do not carry forward

### Requirement: Identifier validation compatibility
The system SHALL preserve the accepted and rejected identifier grammar when equivalent identifier validators are consolidated.

#### Scenario: Same identifier reaches configuration and roster validation
- **WHEN** both surfaces validate the same identifier class
- **THEN** equivalent identifiers receive compatible acceptance or rejection results

### Requirement: Network endpoint compatibility
The system SHALL preserve host and port formatting behavior, including unambiguous IPv6-safe formatting where supported, when endpoint construction is consolidated.

#### Scenario: Endpoint contains an IPv6 host
- **WHEN** a supported IPv6 host and port are formatted as a network endpoint
- **THEN** the result remains unambiguous and compatible with the consuming network interface


### Requirement: Baseline spec traceability gate
The repository SHALL maintain a reviewable mapping from each baseline OpenSpec capability to runtime implementation evidence, regression evidence, and an explicit PASS, PARTIAL, or MISSING status. A capability SHALL NOT be represented as complete solely because the workspace compiles or a control command is accepted.

#### Scenario: Feature compiles but execution path is incomplete
- **WHEN** a baseline capability has types or command parsing but lacks a required observation, policy, validation, transport, or completion-evidence path
- **THEN** its traceability status remains PARTIAL or MISSING
- **AND** operator-facing documentation does not call that capability complete

#### Scenario: Feature is marked complete
- **WHEN** a baseline capability is changed to PASS
- **THEN** the traceability entry identifies concrete implementation evidence
- **AND** the critical baseline scenarios have automated regression evidence or a documented live integration test

### Requirement: Canonical deterministic behavior is reused
The system SHALL provide one canonical deterministic implementation for a semantic operation when that operation is required by multiple missions, policies, or protocol paths. Callers SHALL reuse that implementation rather than maintain behaviorally equivalent local copies.

#### Scenario: Quest work needs movement
- **WHEN** quest execution requires movement to a grounded target or search area
- **THEN** questing delegates to the shared movement/navigation runtime
- **AND** it does not maintain a separate quest-only movement algorithm or packet encoder

#### Scenario: Quest work needs gathering or loot
- **WHEN** a quest objective requires gathering, looting, combat, inventory counting, targeting, or spell casting that is already supported by a shared deterministic subsystem
- **THEN** questing supplies quest-scoped intent/policy to that subsystem
- **AND** the mechanical execution and validation remain in the shared subsystem

### Requirement: Shared deterministic functions have stable contracts
Reusable deterministic functions SHALL have typed inputs and outputs that preserve units, authority requirements, generation/revision semantics, and failure classification across callers. Hidden caller-specific behavior SHALL NOT be selected by mutable global state.

#### Scenario: Multiple callers resolve a live target
- **WHEN** questing and another mission both need nearest-valid-entity selection
- **THEN** they call the same deterministic target-selection primitive with typed filters/policy
- **AND** equivalent inputs produce equivalent selection behavior

### Requirement: Protocol mechanics are centralized
WotLK packet parsing, opcode/body construction, movement-frame construction, quest-dialog encoding, spell-cast encoding, and other protocol mechanics that represent the same wire operation SHALL be implemented once behind a shared protocol boundary. Mission or policy modules SHALL NOT duplicate wire layouts.

#### Scenario: Controlled and ordinary spell work use the same cast protocol
- **WHEN** a normal player cast and a controlled-unit quest cast use the same WotLK cast message family
- **THEN** both use the shared cast encoder with typed mover/target context
- **AND** only the authority/context inputs differ

### Requirement: Semantic differences are explicit, not copied
When two operations look similar but intentionally require different semantics, the difference SHALL be expressed through typed policy, strategy, or domain variants and covered by regression tests. Copying an implementation and changing local constants SHALL NOT be the default mechanism for expressing the difference.

#### Scenario: Quest-item collection differs from quest-item use
- **WHEN** one objective requires collecting an item and another requires using an item on a target
- **THEN** both may reuse shared inventory/target/action primitives
- **AND** the semantic difference is represented by distinct typed objective/action variants
- **AND** collection code is not copied and modified to approximate item-use behavior

