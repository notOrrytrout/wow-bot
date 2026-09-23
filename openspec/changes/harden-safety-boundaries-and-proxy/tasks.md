## 1. Encounter Authorization

- [x] 1.1 Add `EncounterId` and `EncounterAuthorization`, migrate snapshot schema to v5, and replace strategic target snapshot/runtime state.
- [x] 1.2 Install controller decision authorization from exact observed hostile targets with an eight-target cap, deterministic deduplication, primary-target validation, and revision ownership.
- [x] 1.3 Centralize fail-closed offensive legality and migrate targeting, attack, offensive spell, pet, interrupt, pursuit, switching, and AoE paths; remove pet target as authority.
- [x] 1.4 Make grind, group, quest/goal, and PvP voluntary combat create authorization before action enqueue; keep gather non-voluntary.
- [x] 1.5 Clear authorization on completion and lifecycle invalidation and recall pets from unauthorized targets.
- [x] 1.6 Add the encounter authorization regression matrix.

## 2. Dialogue and Execution Authority

- [x] 2.1 Replace `gm_authorized` with typed `PlanOrigin` throughout controller, dialogue, operator API, bridge, tests, and serialization.
- [x] 2.2 Restrict non-GM dialogue to chat and read-only observations; expose durable/gameplay/admin tools only to trusted GM dialogue.
- [x] 2.3 Enforce origin authority in plan validation, agreed-plan persistence, and execution-time validation.
- [x] 2.4 Replace legacy dialogue expectations with forged-plan and non-GM execution rejection tests.
- [x] 2.5 Add independent typed deterministic/LLM decision mode and off/LLM player-dialogue mode, gate model calls and deterministic strategic quest selection, and add mode-matrix tests.

## 3. Proxy and Runtime Hardening

- [x] 3.1 Add and validate `[proxy.limits]` defaults and passthrough world port 8088 with collision/routing checks.
- [x] 3.2 Add global and per-IP pre-auth admission limits before auth/world/single-port/transparent relay task spawn.
- [x] 3.3 Apply one absolute handshake deadline through authentication; configured sessions release permits after parsed auth success, while transparent unknown-account sessions retain pre-auth ownership until authoritative upstream auth-response progress because the response is opaque without the session key.
- [x] 3.4 Implement configured-realm-only rewrite and dedicated transparent-world challenge/auth-session forwarding plus encrypted relay.
- [x] 3.5 Replace supervisor worker unbounded transport with coalesced control state and bounded commands; detach unhealthy workers on saturation.
- [x] 3.6 Add timeout, cap, permit-release, transparent-relay, realm-rewrite, collision, and queue-saturation tests.
- [x] 3.7 Add durable per-bot JSONL action timelines for headless and interactive runs, including plan-source correlation, full action lifecycles, revision/task/encounter context, durations, failures, and important state transitions.
- [x] 3.8 Add typed hunter-pet happiness and explicit WotLK loyalty status, emit semantic pet transitions to JSONL, and emit full pet debug summaries on change and at a bounded periodic interval.

## 4. Deterministic Data

- [x] 4.1 Expand provenance lock with core/database/module/spell-SQL/correction/client/DBC inputs and detected module list.
- [x] 4.2 Extend verification to fail closed on tracked changes and report untracked files separately.
- [x] 4.3 Add `family_root` and serialized `AbilityTags`, reviewed root-to-tag data, and spellbook hydration.
- [x] 4.4 Replace combat-path display-name matching with family/tag checks and add fail-closed semantic tests.

## 5. Behavior-Preserving Refactors

- [x] 5.1 Split proxy world into listener/handshake/relay/observer/handoff/chat/packets modules with public-path compatibility.
- [x] 5.2 Split plugin lifecycle/execution/packet dispatch and action execution domains without behavior changes.
- [x] 5.3 Split quest, reducer, navigation, LLM context/tools, parsing, and world-semantics large modules with re-exports and no behavior change.

## 6. Final Validation

- [x] 6.1 Run focused tests after each behavior phase and module move.
- [x] 6.2 Run `cargo fmt --check`, `cargo check --workspace --locked`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, and `cargo test --workspace --locked`.
- [x] 6.3 Run deterministic provenance verification against the pinned AzerothCore checkout and WotLK build-12340 DBC directory; record environment-only blockers without modifying AzerothCore.
