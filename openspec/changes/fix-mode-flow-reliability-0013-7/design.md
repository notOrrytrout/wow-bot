## Context

The bot has several independent state holders: action execution, activity leases, fishing runtime, collection runtime, reducer snapshot state, static world knowledge, and model-facing tool projection. The remaining `0013.6` defects occur when one layer treats another layer as idle or complete before the whole mode lifecycle is actually safe to stop.

Collection completion currently returns from the execution clock before combat/recovery/loot servicing. Fishing state survives outside normal action execution, but the idle lease cleanup only checks normal actions and can release the Fishing lease during `WaitBobber`. Generic gather navigation filters through `can_harvest`, which intentionally excludes Fishing, so Fishing has no deterministic location-travel path. Fishing classification currently depends on gather game-object names and does not identify ordinary fish item names. Gather skill reporting asks an already eligibility-filtered selector whether the selected node is ineligible, so the impossible-rank branch is unreachable. Collection restrictions are applied to some choice lists but the underlying mission gate can still narrow tools according to Quest/Gather/Goal semantics. The private collection quest ID is enforced only in the executor, after unrelated offers can already be shown to the model.

## Goals / Non-Goals

### Goals

- Keep safety-critical execution active after collection completion until logout is genuinely safe.
- Keep Fishing ownership stable across asynchronous cast, bobber, and loot phases while allowing forced activity to preempt it.
- Require a trusted, location-backed fishing destination before deterministic fishing casts.
- Distinguish fishing travel eligibility from Mining/Herbalism harvest eligibility.
- Classify fishing item names from authoritative generated data rather than string heuristics.
- Detect profession-rank impossibility before the bot enters generic exploration.
- Make collection mode the highest-priority voluntary decision gate.
- Use the private configured collection quest ID to scope model-facing quest choices without serializing it.
- Add state-transition regression coverage and make test success a release condition for `0013.7`.

### Non-Goals

- Do not change WotLK combat or movement protocol behavior.
- Do not expose private quest IDs, creature allowlists, coordinates, or internal world-knowledge identifiers to the model.
- Do not make fishing holes behave like Mining/Herbalism nodes that are directly `GAMEOBJ_USE`d.
- Do not guess a fishing destination when no target-specific trusted location is available.
- Do not redesign Quest, Grind, or Goal mode except where collection mode must take precedence.
- Do not implement code as part of this OpenSpec change proposal.

## Decisions

### 1. Collection completion becomes a drain state, not an execution-clock early return

When collection is complete and `logout_on_complete` is enabled, the runtime will enter a completion-drain condition. New voluntary mission work must stop. Existing voluntary queued work or voluntary movement that can block shutdown must be cancelled or allowed to terminate cleanly. Forced `Death`, `Combat`, and `Recovery` work, plus pending or automatic `Loot`, must continue to be serviced. Safe logout is evaluated after those services run. The logout request is sent at most once when combat is clear, recovery is clear, loot is clear, execution is idle, and no active activity can still mutate combat state.

This keeps the current executor safety checks and avoids creating a second combat loop for shutdown.

### 2. Fishing lease lifetime follows `FishingRuntime`, not normal action idleness

`release_idle_voluntary_lease()` must treat an active Fishing runtime as owned work even when `ActionRuntime` is idle. A Fishing lease remains valid through `EquipPole`, `Cast`, `WaitBobber`, and `UseBobber` until the fishing runtime interrupts or completes its cycle. Forced activity can still preempt Fishing through the normal arbiter. When that happens, Fishing must interrupt and release/lose the old generation rather than silently resume under stale ownership.

### 3. Fishing gets deterministic travel through a separate travel-eligibility path

Mining/Herbalism harvest eligibility and Fishing travel eligibility are different concepts and must not share one `can_harvest` predicate.

Add a gather-travel selector that applies:

- Mining/Herbalism: current profession harvest eligibility.
- Fishing: Fishing skill eligibility and a trusted location-backed Fishing hint.

`NavigateToGatherResource` can remain the public action, but its validation and executor must branch on `GatherKind::Fishing`. For Fishing, reaching the configured discovery/arrival envelope completes the travel action; the executor must not require a clickable fishing game object to hydrate and must not directly use the fishing-hole object. `maybe_run_fishing()` may begin or continue the fishing cast cycle only when the player is already inside the trusted fishing destination envelope. If the player is outside, deterministic gather scheduling owns travel first.

A location-backed resource such as a named pool/school can therefore route deterministically. A fishing item that is classified as Fishing but has no target-specific trusted location must not cause blind casting at the current position; supervisor state should report that a fishing location is unavailable.

### 4. Fishing item classification comes from generated authoritative data

Extend the world-knowledge generator with an additive index of item names that occur in AzerothCore fishing loot data, resolved through item-template names. `gather_kind_for_name()` first keeps its existing gather-node resolution and then checks the generated fishing-item index. Matching entries classify as `GatherKind::Fishing`.

Do not use broad lexical rules such as "name contains fish". This avoids false positives and makes classification reproducible from the server data used to build the knowledge pack.

The classification index is intentionally separate from location hints. If the current server data cannot establish a target-specific trusted location for an open-water fish item, the mode remains correctly classified as Fishing but must report `fishing_location_unavailable` instead of casting in an arbitrary place.

### 5. Profession feasibility uses raw matching hints before eligibility filtering

Add a raw matching-hint view that preserves required skill metadata before `can_harvest`/`can_fish` filtering. The supervisor and deterministic scheduler can then distinguish these states:

- no world-knowledge match,
- skill data not yet hydrated,
- matching work exists but all known candidates exceed current rank,
- at least one eligible candidate exists.

If all same-map trusted candidates exceed the current Mining/Herbalism rank, report profession-rank insufficiency and do not fall through to generic exploration. Apply the equivalent Fishing rank check when a location-backed Fishing hint has a required skill. If an eligible candidate exists, continue normally even when other candidates require a higher rank.

### 6. Collection mode owns the top-level voluntary tool gate

`apply_mission_gate()` must check enabled collection state before branching on the underlying mission. When collection is active, use a dedicated collection gate and return its result directly.

The collection gate must preserve forced/self-defense actions and required logistics, while restricting voluntary work to the collection objective:

- item/money collection: allowed collection combat, approach, loot, inventory/vendor logistics, safe stop/wait, and bounded exploration needed to find allowed targets;
- quest collection: only the configured quest lifecycle, required movement/inspection, self-defense, and logistics;
- completed collection: no new voluntary collection work; runtime completion-drain behavior controls shutdown.

Executor-side collection checks remain as defense in depth.

### 7. Collection quest scope is private but applied before model projection

Add a private `target_quest_id: Option<u32>` field to sanitized `CollectionState`, excluded from serde and schema output in the same way as private creature allowlists. Populate it from `CollectionConfig` only for quest collection.

Add a scope helper used by model-facing quest choice projection and deterministic collection quest flow. It must remove unrelated quest offers before tool schemas/choices are produced. The generic "server offer is collectable" semantic should remain separate from "this offer is authorized by the active collection job" so ordinary Quest mode does not inherit collection restrictions.

Keep the executor's `CollectionConfig::allows_quest()` check unchanged as the final safety backstop.

### 8. Release is test-gated

The implementation must add focused unit/state-machine tests for each transition below and run formatting, compile checks, and the full Rust test suite before changing build labels to `0013.7` or producing the release ZIP.

## Risks / Trade-offs

- Cancelling voluntary work during collection drain could discard a harmless queued action. This is acceptable because collection is already complete; safety work has priority.
- Holding the Fishing lease longer can reduce voluntary mode switching during a cast cycle. Forced activity still preempts it, and the lease ends on interrupt/loot completion, so safety is not reduced.
- Static fishing-hole coordinates can be in water. The fishing travel branch therefore stops within an arrival envelope and does not path to interaction range or use the hole object directly.
- Adding fishing-item metadata increases the bundled knowledge pack. Use a compact deduplicated name/index representation and keep the schema additive when possible.
- Correct classification without a trusted target-specific location can leave an open-water fish mission waiting. This is preferable to farming an unrelated location or casting blindly. A future area-aware fishing-loot location capability can extend this safely.
- A collection-specific gate can accidentally hide emergency actions if it is too narrow. Tests must verify self-defense, death/recovery, loot, and required logistics remain available.

## Migration Plan

1. Add code and tests without changing public collection configuration syntax.
2. Extend and regenerate bundled world knowledge with the fishing-item classification index.
3. Verify existing knowledge packs either remain readable or fail with an explicit version/schema error if a format bump is required.
4. Run targeted tests, `cargo fmt --check`, `cargo check`, and full `cargo test` in a Rust-enabled environment.
5. Only after all checks pass, update build-stage/channel labels to `0013.7` and package the release.
6. Rollback is a code/data revert to `0013.6`; no user configuration migration is required.
