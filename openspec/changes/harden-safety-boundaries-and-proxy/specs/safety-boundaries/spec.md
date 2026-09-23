## ADDED Requirements

### Requirement: Explicit encounter authorization
The system SHALL represent voluntary combat ownership with an encounter UUID, primary target, deterministic allowed-target set, mission revision, and permission revision. A model-created allowed-target set SHALL contain one through eight currently observed targets, SHALL contain the primary target, and SHALL not contain duplicate GUIDs.

#### Scenario: Unrelated hostile is nearby before combat
- **WHEN** no encounter authorization contains the hostile and it is not already attacking the player, owned pet, or active group member
- **THEN** SetTarget, Attack, offensive spell, pet attack, interrupt, pursuit, target switching, and AoE logic reject that hostile as a voluntary pull

#### Scenario: Authorized first pull
- **WHEN** a current encounter authorization contains a living valid hostile and revisions still match
- **THEN** the voluntary offensive action may target that hostile subject to ordinary action validation

### Requirement: Pet state is not combat authority
A pet-selected target SHALL NOT expand the encounter allowed-target set or make an unrelated hostile legal. An owned pet that is attacking an unauthorized hostile SHALL be recalled with Follow/Passive behavior when possible.

#### Scenario: Pet independently selects another hostile
- **WHEN** the pet target points at a living hostile that is outside encounter authorization and is not already attacking the group
- **THEN** the hostile remains illegal and the pet is recalled instead of authorizing a new pull

### Requirement: Encounter completion is terminal
Encounter authorization SHALL clear on mission revision change, permission or stage change, death, world reset, invalid target state, or after no allowed or already-engaged hostile remains. Completion SHALL NOT select another nearby hostile automatically.

#### Scenario: Last encounter hostile dies
- **WHEN** no allowed or already-engaged hostile remains alive
- **THEN** the authorization clears and combat control returns to strategic idle without selecting a replacement enemy

### Requirement: Typed plan origin
Every executable plan SHALL identify one of `controller`, `trusted_gm_dialogue`, `untrusted_dialogue`, or `operator_api`. Authority SHALL be enforced independently at tool exposure, plan validation, durable-intent persistence, and execution.

#### Scenario: Non-GM dialogue requests gameplay
- **WHEN** an untrusted dialogue plan contains a gameplay-changing, movement, quest-write, invite, targeting, combat, durable-plan, or administrative operation
- **THEN** validation or execution rejects it even if the tool became exposed accidentally

#### Scenario: Trusted GM dialogue requests gameplay
- **WHEN** trusted GM dialogue requests an otherwise legal gameplay action
- **THEN** the action can proceed under the same ordinary safety validators as controller/operator actions

### Requirement: Independent decision and dialogue modes
The system SHALL configure autonomous decision ownership independently from player dialogue. Deterministic decision mode SHALL make no autonomous controller-model requests. LLM decision mode SHALL let the controller select grounded strategic work while Rust retains validation and mechanical execution. Dialogue-off mode SHALL make no player-dialogue model requests, while LLM dialogue mode SHALL preserve the typed dialogue authority boundary.

#### Scenario: Deterministic decisions with LLM dialogue
- **WHEN** decision mode is deterministic and player dialogue mode is LLM
- **THEN** Rust selects autonomous work and player dialogue may use the dialogue model

#### Scenario: LLM decisions with dialogue disabled
- **WHEN** decision mode is LLM and player dialogue mode is off
- **THEN** grounded autonomous controller planning may use the model and player chat does not start dialogue inference

### Requirement: Bounded proxy pre-authentication resources
Auth, normal world, transparent-world, and single-port classifier listeners SHALL enforce configured global and per-IP pre-authentication limits before task spawn. The downstream authentication phase SHALL use one configured handshake deadline. Configured-account sessions SHALL release the pre-auth permit only after parsed authoritative authentication success. Transparent unknown-account sessions cannot classify the encrypted authentication result without the unknown account session key, so client-controlled authentication bytes SHALL NOT release admission ownership; the permit SHALL remain held until the authoritative upstream server produces authentication-response progress.

#### Scenario: Silent connection flood
- **WHEN** clients connect and do not complete authentication
- **THEN** active pre-auth work remains bounded and stale handshakes time out

### Requirement: Transparent unknown-account world passthrough
When unknown-account passthrough is enabled, the proxy SHALL keep SRP authentication end to end and rewrite only the configured AzerothCore realm to the dedicated transparent-world listener. That listener SHALL forward the upstream world challenge and downstream authentication-session bytes unchanged before relaying the encrypted session bidirectionally.

#### Scenario: Unknown account completes world login
- **WHEN** an unknown account completes upstream auth and connects to the rewritten configured realm
- **THEN** the upstream world server receives its original authentication-session exchange and the encrypted session is relayed without requiring a proxy-derived session key

#### Scenario: Realm list contains another realm
- **WHEN** the upstream auth server returns a realm that is not the configured AzerothCore route
- **THEN** that realm address remains unchanged

### Requirement: Bounded worker transport
Supervisor control state SHALL be coalesced and worker commands SHALL use a bounded ordered queue. If a plan/shutdown command cannot be queued because the worker is unhealthy/full, the supervisor SHALL detach that worker instead of silently dropping the command.

#### Scenario: Worker stops consuming commands
- **WHEN** the bounded command queue reaches capacity
- **THEN** the worker is marked unhealthy/detached and no accepted plan is silently discarded

### Requirement: Durable per-bot action timeline
Headless and interactive runs SHALL append a separate structured JSONL timeline for each configured bot. The timeline SHALL record dispatched plan source and summary, every reported action lifecycle state, action parameters, decision and tool correlation IDs, snapshot/mission/permission/task/encounter context, elapsed duration, typed failure details, and important world-state transitions. Routine packet and movement-heartbeat traffic SHALL remain outside this timeline. Movement heartbeats MAY be recorded in a separate private per-bot diagnostics JSONL stream that is not treated as the durable action audit. Each append SHALL be flushed and synchronized before it is accepted as durable. If a timeline append fails, the supervisor SHALL mark that bot's audit stream degraded and SHALL refuse additional plan dispatches for that bot for the remainder of the supervisor session.

#### Scenario: Headless action fails
- **WHEN** a headless worker reports queued, started, waiting, and failed states for an action
- **THEN** the bot's timeline contains ordered records for all four states with one action ID, its decision source, revision context, elapsed duration, failure kind, and failure reason

#### Scenario: Deterministic action has no model decision ID
- **WHEN** Rust orchestration creates an action without a dispatched model plan
- **THEN** the timeline classifies its decision source as deterministic Rust instead of leaving the source ambiguous

#### Scenario: Timeline append fails
- **WHEN** a plan or lifecycle record cannot be appended and synchronized
- **THEN** the audit stream is marked degraded, the error is surfaced, and no additional plan is dispatched for that bot in the current supervisor session

### Requirement: Observable pet welfare state
The system SHALL derive the AzerothCore WotLK hunter-pet happiness category from authoritative `POWER_HAPPINESS` values using the pinned server thresholds. Pet diagnostics SHALL explicitly report loyalty as `not_applicable_wotlk` because this server build exposes no pet-loyalty state. The per-bot timeline SHALL record meaningful pet-status and happiness-category transitions, and debug logging SHALL publish the complete current pet summary on change and at a bounded periodic interval.

#### Scenario: Hunter pet happiness drops from happy to content
- **WHEN** the observed happiness crosses from at least 666000 to below 666000
- **THEN** the timeline records a pet-status transition with raw current/maximum happiness and the derived `content` category

#### Scenario: Pet state remains unchanged
- **WHEN** repeated snapshots contain the same meaningful pet state
- **THEN** the JSONL timeline does not repeat the transition, while debug mode emits at most one periodic summary per configured interval

### Requirement: Deterministic spell provenance
The repository SHALL lock and verify the pinned AzerothCore checkout commit, database-declared revision, ACDB version/cache ID, installed module names/commits, relevant spell SQL hashes, `SpellInfoCorrections.cpp`, WotLK client build 12340, and required DBC hashes. Verification SHALL fail closed when a tracked input changes and SHALL report untracked files separately.

#### Scenario: Locked spell SQL changes
- **WHEN** a tracked `spell_*` source hash differs from the provenance lock
- **THEN** provenance verification fails with the changed path

### Requirement: Semantic combat ability classification
Runtime abilities SHALL expose their spell-family root and additive semantic tags. Combat policy SHALL use family roots/tags rather than display-name matching for behavior selection. Unknown spell IDs SHALL not gain semantic capabilities from their display names.

#### Scenario: Unknown spell has a misleading familiar name
- **WHEN** an unknown spell ID has text that matches a known interrupt or buff name
- **THEN** combat policy does not classify it as that semantic ability unless the family/tag catalog explicitly does so
