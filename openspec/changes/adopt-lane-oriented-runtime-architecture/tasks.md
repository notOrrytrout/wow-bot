# Tasks

## 1. Domain and control types

- [ ] 1.1 Add strong types for state, mission, permission, worker, ownership, movement, and activity generations/revisions.
- [ ] 1.2 Add typed pause reasons and remove any single-Boolean pause assumptions at the runtime boundary.
- [ ] 1.3 Define versioned worker/proxy/supervisor IPC DTOs with explicit request IDs and generation stamps.
- [ ] 1.4 Add compatibility tests for control-protocol encode/decode and version rejection.

## 2. Tentacli adapter boundary

- [ ] 2.1 Add `wow-tentacli-adapter` and make it the only ordinary bot-runtime crate that imports Tentacli WotLK object/protocol types directly.
- [ ] 2.2 Pin the migration reference to Tentacli commit `354a9888855d2159029943e8095c79539ff9796d` / version `15.3.1`, or an exact equivalent source with verified adapter parity.
- [ ] 2.3 Translate Tentacli object creation/update/removal, movement, names, and relevant packets into `ProtocolObservation` values.
- [ ] 2.4 Add adapter fixture tests for object lifecycle, position, unit/player/game-object accessors, and malformed/incomplete observations.
- [ ] 2.5 Prevent Tentacli context/object structures from leaking into policy and execution public APIs.

## 3. Reducer-owned authoritative state

- [ ] 3.1 Make one reducer entry point the mutation boundary for packet-derived dynamic gameplay state.
- [ ] 3.2 Separate authoritative state, static world knowledge, memory evidence, and configuration types.
- [ ] 3.3 Make snapshots immutable projections carrying `StateRevision`.
- [ ] 3.4 Add stale-snapshot and entity-removal tests that exercise final action validation.

## 4. Lane engine

- [ ] 4.1 Introduce one `LaneEngine` task per configured worker lane and move mutable mission/task/action/movement/activity ownership into it.
- [ ] 4.2 Convert helper tasks to message/result producers instead of direct lane-state mutators.
- [ ] 4.3 Add generation-stamped completion handling for route, model, memory, and other asynchronous work.
- [ ] 4.4 Use cancellation for prompt wake-up only; keep generation comparison as the stale-result safety fence.
- [ ] 4.5 Add concurrency tests proving an older async result cannot overwrite newer state.

## 5. Activity and action execution

- [ ] 5.1 Implement one per-lane activity arbiter with typed leases and `ActivityGeneration`.
- [ ] 5.2 Migrate movement, combat, loot, gathering, fishing, recovery, and maintenance conflicts to the arbiter.
- [ ] 5.3 Introduce `ProposedAction`, `PreparedAction`, validation outcome, and private-constructor `SendableAction` boundaries.
- [ ] 5.4 Ensure `NeedsMovement` cannot enter the transport API.
- [ ] 5.5 Preserve one terminal result per logical action and existing typed bounded retry behavior.

## 6. Configured proxy session actor

- [ ] 6.1 Make one `ConfiguredAccountSession` actor the exclusive upstream writer for each configured account.
- [ ] 6.2 Keep ownership generation, transition ticket, player attendance, bot-assistance state, worker generation, movement epoch, and locomotion owner in that actor.
- [ ] 6.3 Add the final proxy transport-authority gate immediately before upstream writes.
- [ ] 6.4 Reject stale worker/ownership/movement generations without forwarding gameplay bytes.
- [ ] 6.5 Preserve the separate transparent unknown-account world relay and its existing authentication semantics.
- [ ] 6.6 Validate configured proxy behavior against the supplied AzerothCore reference commit `41f475e9da33a2eeea243f5641d170cbfa11ab4a` where protocol behavior is material.

## 7. Typed IPC migration

- [ ] 7.1 Replace any worker/proxy synthetic WoW connection or direct shared-memory gameplay boundary with typed IPC.
- [ ] 7.2 Use coalesced latest-value delivery for control state that is explicitly safe to coalesce.
- [ ] 7.3 Use bounded ordered queues for commands and action/movement requests requiring individual delivery.
- [ ] 7.4 Preserve saturation behavior that detaches or marks unhealthy workers instead of silently dropping accepted commands.

## 8. Movement runtime

- [ ] 8.1 Separate route computation service from logical movement ownership.
- [ ] 8.2 Keep movement ID and task ownership stable across same-objective replans.
- [ ] 8.3 Stamp route requests/results so older completions cannot replace current route geometry.
- [ ] 8.4 Treat packet-derived position as authoritative and predicted locomotion as intent.
- [ ] 8.5 Add player-takeover race tests proving movement-epoch advancement fences queued bot locomotion without deleting the mission.

## 9. Workspace dependency cleanup

- [ ] 9.1 Create or align `wow-domain`, `wow-state`, `wow-tentacli-adapter`, `wow-engine`, `wow-proxy`, `wow-control-proto`, `wow-supervisor`, and supporting crates with acyclic dependency direction.
- [ ] 9.2 Prevent policy/domain crates from importing proxy transport or Tentacli runtime types.
- [ ] 9.3 Keep gameplay feature organization inside policy/engine crates unless a feature has a real dependency, security, process, or build boundary.
- [ ] 9.4 Document that this change supersedes only the Tentacli dependency-source decisions of the two earlier Tentacli changes; retain their behavior-preserving lessons and history.

## 10. Validation

- [ ] 10.1 Add lane-isolation tests for worker crash, restart, saturation, and proxy continuity.
- [ ] 10.2 Add generation-race tests for ownership takeover, worker replacement, movement handoff, activity preemption, route completion, and model completion.
- [ ] 10.3 Add tests proving only final validation can create a transport-accepted sendable action.
- [ ] 10.4 Add tests proving the proxy is the exclusive configured upstream writer.
- [ ] 10.5 Run `cargo fmt --check`, `cargo check --workspace --locked`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, and `cargo test --workspace --locked` once the Rust workspace is present.
- [ ] 10.6 Run `openspec validate --changes` and resolve all structural/spec validation failures.
