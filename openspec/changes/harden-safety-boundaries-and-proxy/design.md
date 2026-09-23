## Context

The project already has mission and permission revisions, controller decision UUIDs, a short-lived strategic target authorization, GM-aware dialogue messages, proxy auth/world endpoints, and generated spell-rank provenance. The problem is that these controls are incomplete or duplicated. The implementation must replace them with shared fail-closed primitives instead of layering more heuristics on top.

The target server is AzerothCore WotLK 3.3.5a. The implementation plan identifies checkout commit `ab99938ab6474178f81afc55a171c74d934d5385` and database-declared core revision `8b8ba243d8f7` as separate provenance facts. The WotLK client build is 12340.

## Goals / Non-Goals

**Goals:**

- Make voluntary combat ownership explicit and inspectable.
- Keep already-engaged self-defense legal without letting pet selection create authority.
- Treat model output as untrusted input and enforce authority below model/tool selection.
- Bound unauthenticated network and worker resources.
- Make unknown-account auth/world passthrough internally consistent.
- Fail closed when deterministic spell inputs differ from the pinned source set.
- Move combat decisions from display-name matching to semantic family/tag data.

**Non-Goals:**

- Modify or build AzerothCore.
- Create a pull planner inside the combat rotation.
- Make non-GM dialogue a gameplay-control channel.
- Invent server observations that are not present in snapshots.
- Mix behavior changes with module moves.

## Decisions

### Encounter authorization is a snapshot-visible value object

Add `EncounterId(Uuid)` and `EncounterAuthorization { encounter_id, primary_target, allowed_targets, mission_revision, permission_revision }`. `allowed_targets` is deduplicated and sorted by GUID representation. A model-created set is limited to eight observed hostile targets and cannot be empty. The primary target must be in the set.

Use the controller decision UUID as the encounter ID. Rust resolves model choices into exact observed GUIDs before installing authorization. Authorization is cleared on mission, permission/stage, death/world reset, invalid target state, or encounter completion.

### One fail-closed offensive legality function

A voluntary offensive target is legal only when it is in the current encounter authorization and revisions match. An already-engaged attacker remains legal self-defense when it is attacking the player, an owned pet, or an active group member. Battleground hostile-player policy remains a separate explicit branch. Breakable crowd control is rejected before those branches.

Every offensive action path consumes this same function: target selection, weapon/spell attack, pet attack, interrupt, pursuit, target switching, and AoE envelope validation. Pet `UNIT_FIELD_TARGET` and locally latched pet attack assignments are state, not authority.

### Plan authority uses a typed origin

Replace `gm_authorized` with `PlanOrigin::{Controller, TrustedGmDialogue, UntrustedDialogue, OperatorApi}`. Origin determines capabilities. Untrusted dialogue can chat and use read-only observation only. Trusted GM dialogue can request durable intent and gameplay tools. Controller and operator API retain their existing non-dialogue responsibilities.

Tool hiding is convenience only; validators and executors independently reject actions that exceed the origin. Agreed-plan persistence accepts trusted GM dialogue only.

### Decision and player-dialogue model use are independent

Add typed `decision_mode` values `deterministic` and `llm`, and typed `player_dialogue_mode` values `off` and `llm`. Deterministic decision mode disables autonomous controller inference and keeps strategic selection in Rust. LLM decision mode gives grounded strategic selection to the controller while Rust retains protocol handling, safety validation, navigation, and combat mechanics. Dialogue mode independently gates player-triggered dialogue inference and does not change plan-origin authority.

### Proxy admission is bounded before task spawn

Add `[proxy.limits]` defaults: 128 pre-auth connections per listener, 8 per source IP, 10,000 ms handshake timeout, and worker command capacity 64. A listener acquires global and per-IP permits before spawning work. The handshake timeout ends after downstream auth succeeds, so authenticated world sessions are not subject to the pre-auth cap.

### Unknown accounts use a dedicated transparent world listener

Keep auth SRP passthrough end to end. For only the configured AzerothCore realm, rewrite the advertised world endpoint to `proxy_computer_ip:passthrough_world_port` (default 8088). The listener connects to the configured upstream world endpoint, forwards the upstream challenge and downstream `CMSG_AUTH_SESSION` exchange unchanged under the handshake timeout, then uses `copy_bidirectional` for the encrypted session. Other realms remain unchanged.

Startup rejects listener port collisions and passthrough configuration without a valid upstream world endpoint.

### Worker transport separates state from ordered commands

Mission/stage/pause/resume are coalesced through a Tokio watch-style control channel. Welcome/plan/shutdown use a bounded command channel. A full command queue detaches the unhealthy worker instead of silently losing a plan.

### Runtime action diagnostics use a separate JSONL timeline

Create one append-only JSONL file per bot under the private runtime directory. Register every supervisor-dispatched plan before delivery so later worker outcomes can be correlated with typed plan origin and summary. Outcomes without a matching plan are classified as deterministic Rust work. Record lifecycle duration from the first observed state, include current trusted snapshot revisions and task/encounter context, and emit compact records for login, life, combat, mission, task, encounter, stage, worker-connect, and worker-disconnect transitions. Keep packet frames out of the durable action timeline. Movement heartbeats may be written to a separate private per-bot diagnostics JSONL stream so movement cadence and terrain reconciliation can be analyzed without polluting the action audit timeline.

Derive hunter-pet happiness with AzerothCore's three fixed WotLK bands: below 333000 is unhappy, 333000 through 665999 is content, and 666000 or more is happy. Do not invent loyalty: the pinned server has happiness persistence and no loyalty field or state machine, so diagnostics serialize `not_applicable_wotlk`. Compare compact semantic pet keys for JSONL transitions to avoid logging every health or happiness tick, but include raw values in each emitted payload. Emit the same complete summary at debug level when that semantic key changes and once every 60 seconds while snapshots continue.

### Provenance locks effective spell inputs

Record core checkout commit, database-declared core revision, ACDB version/cache ID, module names/commits, hashes for relevant `spell_*` SQL sources, `SpellInfoCorrections.cpp`, client build 12340, and required DBC hashes. Verification fails closed for changed tracked inputs or module commits. Untracked files are reported separately.

### Ability semantics are hydrated once

Extend `Ability` with `family_root` and serialized additive `AbilityTags`. Generate reviewed root-to-tag data. Hydrate family roots/tags when the spellbook is built. Combat policy uses IDs/tags and keeps names only for display/diagnostics. Unknown IDs remain fail closed.

### Refactoring follows behavior changes

After tests cover the new behavior, split proxy world and the remaining large modules into responsibility-based submodules. Preserve existing public paths through re-exports and do not alter behavior in the move commits/files.

## Risks / Trade-offs

- Existing callers may submit voluntary attacks without installing authorization. They must be migrated rather than grandfathered, otherwise the boundary is not meaningful.
- Snapshot schema 5 is intentionally incompatible with mixed old/new worker processes; supervisor and workers must restart together.
- Transparent world relay cannot inspect encrypted traffic after authentication. This is intentional for unknown-account passthrough.
- Semantic-tag coverage will initially be narrower than name heuristics. Unknowns fail closed, so this can reduce automation until reviewed tags are added.
