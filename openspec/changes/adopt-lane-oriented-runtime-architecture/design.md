# Design

## Reference inputs

This architecture is based on the supplied reference snapshots:

- AzerothCore WotLK archive commit: `41f475e9da33a2eeea243f5641d170cbfa11ab4a`.
- Tentacli `primary` archive commit: `354a9888855d2159029943e8095c79539ff9796d`.
- The supplied Tentacli snapshot declares package version `15.3.1`.

These are reference inputs for protocol and adapter behavior. wow-bot remains responsible for its own semantic state, safety policy, ownership policy, and execution fencing.

## Architectural style

Use a lane-oriented actor architecture with explicit ports between protocol, state, policy, execution, and session ownership.

The central rule is:

> One task owns mutable gameplay state for one configured bot lane. Other tasks communicate with that owner through typed messages or immutable snapshots.

Do not make shared `Arc<Mutex<_>>` or `Arc<RwLock<_>>` graphs the primary gameplay-state architecture. Synchronous or asynchronous helper tasks may use internal synchronization where required, but they do not mutate lane gameplay state directly and no synchronous lock may be held across an `.await`.

## Process topology

The supervisory process contains the fleet supervisor and the control proxy. Each configured lane has a separately supervised worker process.

```text
supervisory process
├── FleetSupervisor
├── ControlProxy
│   ├── ConfiguredAccountSession[A]
│   ├── ConfiguredAccountSession[B]
│   └── TransparentSession[unknown accounts]
├── worker process A
├── worker process B
└── worker process C
```

A worker failure is lane-local. Restarting worker A does not reset ownership, audit state, or gameplay state for B or C.

## Configured account session authority

For each configured account, `ConfiguredAccountSession` owns the authoritative upstream AzerothCore WorldSession and is the only component that writes gameplay bytes to that upstream session.

The session actor owns at least:

- ownership generation,
- active transition ticket,
- player connection count,
- current attended-control state,
- explicit bot-assistance state,
- movement epoch and locomotion owner,
- worker generation,
- authoritative upstream connection state,
- handoff/reclaim timers and barriers.

The worker never owns a second authoritative server socket for the same configured account.

Configured account traffic is terminated, framed, inspected, and re-emitted at the proxy boundary. It is not modeled as one raw encrypted byte stream shared by downstream and upstream connections.

Unknown-account transparent passthrough remains a separate implementation path and preserves its existing end-to-end authentication and encrypted-relay requirements.

## Worker/proxy IPC

Workers communicate with the proxy using a typed, versioned control protocol. Do not create a synthetic WoW authentication or WorldSession between worker and proxy.

Representative direction:

```text
WorkerToProxy
- Action(ActionRequest)
- Movement(MovementRequest)
- Query(SessionQuery)
- Ready(WorkerGeneration)
- ShutdownAck

ProxyToWorker
- Observation(ProtocolObservation)
- OwnershipChanged(OwnershipSnapshot)
- MovementFence(MovementEpoch)
- SessionState(SessionSnapshot)
- ActionResult(ActionResult)
```

Commands requiring ordering and individual delivery use bounded queues. Latest-value state such as mission revision, activation stage, pause-reason set, permission revision, and desired execution state may use coalesced/latest-value transport.

Queue saturation is treated as an unhealthy worker or unhealthy control path according to the existing bounded-worker-transport requirements; accepted commands are not silently discarded.

## Tentacli boundary

Tentacli is a WotLK protocol/client implementation dependency, not the owner of wow-bot architecture.

Create a narrow `wow-tentacli-adapter` boundary. Only this adapter imports Tentacli directly for bot runtime protocol/object interpretation. Other wow-bot domain, policy, state, and execution crates depend on wow-bot types instead of Tentacli types.

The adapter converts Tentacli packet/object/lifecycle information into typed wow-bot observations:

```text
Tentacli packet/object API
        ↓
wow-tentacli-adapter
        ↓
ProtocolObservation
        ↓
AuthoritativeState reducer
```

Tentacli `ObjectMap`, accessors, lifecycle metadata, and positions may be used as canonical decoded protocol inputs. They do not become the semantic `AuthoritativeState` and do not grant action authority by themselves.

The supplied Tentacli `15.3.1` snapshot is the reference API for this change. Pin an exact source revision during migration. A later move to a published package is allowed only after required API and behavior parity is verified through the adapter tests; the rest of wow-bot must not need source changes for that switch.

This decision supersedes the direct dependency-source choices in `refactor-tentacli-dependency` and `use-local-tentacli-object-accessors` without deleting those historical change artifacts.

## Authoritative state reducer

`AuthoritativeState` is owned by the lane engine and modified only through protocol/domain observations.

```rust
fn reduce(
    state: &mut AuthoritativeState,
    observation: ProtocolObservation,
) -> StateDelta;
```

Keep four evidence classes separate:

- `AuthoritativeState`: current packet-derived dynamic truth.
- `WorldKnowledge`: generated/static server and client knowledge.
- `MemoryEvidence`: historical observations and learned context.
- `Configuration`: operator policy and fixed runtime configuration.

Static knowledge or model output can inform planning but cannot directly assert current dynamic state.

Snapshots are immutable projections carrying a `StateRevision`. Consumers may share them, but consumers cannot mutate authoritative state through a snapshot.

## Generation model

Use separate strong types for independent invalidation domains. At minimum:

- `StateRevision`
- `MissionRevision`
- `PermissionRevision`
- `WorkerGeneration`
- `OwnershipGeneration`
- `MovementEpoch`
- `ActivityGeneration`

Do not replace these with one generic generation counter.

A work item carries only the generations relevant to its validity. Completion is accepted only when those stamps still match current authority.

## Lane engine

One `LaneEngine` task owns the lane's mutable gameplay runtime. It contains or owns the state machines for:

- authoritative state reduction,
- mission/task runtime,
- pause reasons and activation stage,
- activity arbitration,
- action lifecycle,
- movement lifecycle,
- fishing/gathering/recovery state where applicable,
- group/encounter authorization state,
- relevant revisions and generations.

Policy modules receive immutable state/snapshot inputs and return typed intents or constraints. They do not write sockets or mutate authoritative state.

## Async work and stale completion

Potentially slow work such as route computation, model inference, static-data queries, or durable-memory operations may run outside the lane engine. The request and completion carry the logical identity and relevant generation stamps.

The lane engine discards a completion when its identity or generation is stale. Cancellation tokens may be used for prompt liveness, but cancellation is not the safety fence; generation comparison is the safety fence.

## Activity arbiter

Use one per-lane activity arbiter for incompatible state-changing activities such as movement, combat, loot, gathering, fishing, recovery, and maintenance.

The arbiter is a domain state machine, not a generic mutex or semaphore. An activity lease contains an activity generation. Forced work may preempt voluntary work by advancing that generation. Late results from the preempted generation are rejected.

Temporary preemption preserves recoverable task ownership according to the existing lifecycle specification. Terminal replacement drains the old work.

## Action pipeline and typestate boundary

Action flow is:

```text
StrategicIntent
  → ProposedAction
  → PreparedAction
  → final worker validation
       ├── NeedsMovement
       ├── Rejected(ActionFailure)
       └── SendableAction
             → proxy transport gate
             → upstream WorldSession
```

`SendableAction` has no public unchecked constructor. Only final worker validation can produce it.

`NeedsMovement` is not a sendable variant. Movement preparation completes first and the action must pass final validation again.

The worker-side final validation checks semantic and mechanical legality immediately before submission, including the existing state, mission, permission, stage, origin, encounter, target, and mechanical rules.

The proxy performs an additional transport-authority gate immediately before the upstream write. It checks at least worker generation, ownership generation, current source permission, and movement epoch for locomotion-sensitive commands.

The two gates solve different races: the worker knows gameplay legality; the proxy knows whether this worker is still allowed to write this session.

## Movement architecture

Route computation and physical movement execution are separate.

The navigation service returns route geometry. The lane movement runtime owns the logical movement identity, destination/objective, current route, waypoint state, timing, retry state, and relevant movement generation.

A replan replaces route geometry without replacing logical movement identity. Predicted movement is intent, not authoritative position. Packet-derived observations update authoritative position.

Player physical movement advances/fences bot locomotion according to the existing player-handoff requirements without replacing the durable mission.

## Pause state

Represent pause state as independent typed reasons rather than one Boolean. Removing one reason does not clear another reason.

Representative reasons include operator pause, player control, safety, startup gate, handoff, and shutdown.

## Workspace dependency direction

Target dependency direction:

```text
wow-domain
   ↑
   ├── wow-state
   ├── wow-policy
   ├── wow-control-proto
   └── wow-tentacli-adapter
           ↑
        tentacli

wow-engine → wow-domain + wow-state + wow-policy + wow-control-proto
wow-proxy  → wow-domain + wow-control-proto + protocol/crypto support
wow-supervisor → wow-domain + wow-control-proto
```

A practical workspace may use these packages:

```text
crates/
├── wow-domain/
├── wow-state/
├── wow-tentacli-adapter/
├── wow-policy/
├── wow-navigation/
├── wow-engine/
├── wow-proxy/
├── wow-control-proto/
├── wow-supervisor/
└── wow-infra/

apps/
├── wow-supervisor/
└── wow-worker/
```

Do not create one crate per gameplay feature solely for organizational purposes. Crate boundaries should primarily express dependency, security, protocol, process, or build boundaries.

## Observability

Keep the durable per-lane action audit separate from high-frequency diagnostics.

The action audit is lossless/durable according to the existing baseline. Diagnostics such as movement heartbeats may be bounded, aggregated, or throttled. One noisy lane must not serialize diagnostics or gameplay for the fleet.

## Non-goals

- Do not rewrite route geometry algorithms solely for this architecture change.
- Do not move gameplay semantic state into Tentacli.
- Do not make the proxy decide class combat rotations or quest strategy.
- Do not merge transparent unknown-account passthrough into configured-account termination.
- Do not replace generation fences with task cancellation alone.
- Do not change existing gameplay policy merely to complete the structural migration.

## First-run network topology

The supervisor bootstrap must treat three addresses as independent concepts: the upstream AzerothCore host, the proxy listener bind address, and the proxy address advertised to stock clients. On first run, if these values are not already valid, setup asks whether AzerothCore runs on the same machine and whether clients on other computers will connect to the bot server. Local AzerothCore may use loopback upstream. Remote clients require a non-loopback listener and a reachable advertised host. The validated selections are persisted so subsequent startup is non-interactive unless configuration becomes invalid.
