# Adopt lane-oriented runtime architecture

## Why

The existing specifications define strong behavioral boundaries for per-account ownership, packet-derived authority, generation fencing, action validation, handoff, bounded worker transport, and lane-local failure recovery. Those requirements now need one explicit Rust architecture so implementation does not distribute authority across shared async state, duplicate protocol responsibilities, or let the network library become the gameplay state owner.

The supplied implementation references also clarify the boundary. AzerothCore is the server/protocol authority for the configured WotLK environment. Tentacli provides useful WotLK packet and object decoding, but its general client/plugin runtime is not the correct owner for wow-bot lane state or proxy session authority.

## What Changes

- Define one lane-oriented worker runtime for each configured bot lane. One lane engine owns mutable gameplay state for that lane.
- Keep the supervisor and control proxy in the supervisory process and keep worker failure and restart lane-local.
- Make the configured-account proxy session actor the exclusive writer to the authoritative upstream AzerothCore WorldSession.
- Use typed worker/proxy IPC instead of creating a second synthetic WoW session between worker and proxy.
- Put Tentacli behind a narrow adapter that converts decoded WotLK protocol/object data into wow-bot `ProtocolObservation` values. Tentacli state is not wow-bot authoritative semantic state.
- Keep packet-derived gameplay state in a reducer-owned `AuthoritativeState`; snapshots are immutable projections only.
- Separate state, mission, permission, worker, ownership, movement, and activity generations instead of collapsing them into one epoch.
- Require two final transmission gates: worker-side semantic/mechanical validation and proxy-side session/ownership fencing immediately before upstream write.
- Make non-sendable preparation results such as `NeedsMovement` structurally distinct from `SendableAction`.
- Centralize incompatible gameplay execution in one per-lane activity arbiter with generation-based preemption safety.
- Stamp asynchronous route/model/static-data results with the relevant identity and generations and discard stale completion.
- Keep configured-account termination and transparent unknown-account passthrough as separate proxy paths.
- Use coalesced state channels only for latest-value control state and bounded ordered queues for commands that require individual delivery.
- Supersede the dependency-source decisions in `refactor-tentacli-dependency` and `use-local-tentacli-object-accessors`; preserve those changes as historical records.

## Capabilities

### New Capabilities

- `lane-runtime-architecture`: Rust ownership, process, IPC, adapter, validation, and session-authority architecture for configured bot lanes.

### Modified Capabilities

None. This change constrains implementation architecture while preserving the current observable behavioral specifications.

## Impact

- Workspace/crate layout and dependency direction.
- Worker runtime ownership and async task structure.
- Proxy configured-session ownership and upstream write path.
- Worker/proxy/supervisor IPC schemas.
- Tentacli integration and source pinning.
- Authoritative state reducer and immutable snapshot projection.
- Action preparation/final validation APIs.
- Movement, activity preemption, and stale-result fencing.
- Integration and concurrency tests for lane isolation, handoff, restart, and stale async completion.
