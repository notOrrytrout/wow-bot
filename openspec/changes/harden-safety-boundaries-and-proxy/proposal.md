## Why

The bot has several safety concepts, but important authority remains represented by heuristics: one short-lived strategic target GUID, a `gm_authorized` Boolean inside plans, unbounded pre-authentication socket tasks, and an unbounded worker command channel. Unknown-account auth passthrough also rewrites clients toward a world endpoint that cannot authenticate the original SRP session. Deterministic combat data currently locks only part of the effective AzerothCore spell inputs.

These weaknesses can permit voluntary pulls outside explicit encounter ownership, allow untrusted dialogue to request durable or gameplay-changing actions, exhaust proxy or supervisor resources, break unknown-account passthrough, and make combat semantics drift from the pinned server build.

## What Changes

- Replace the single strategic target with a typed encounter authorization that carries an encounter UUID, primary target, deterministic allowed-target set, and mission and permission revisions.
- Require that authorization before every voluntary offensive action. Keep self-defense and battleground authorization separate and explicit. Pet-selected targets never grant authority.
- Replace `gm_authorized` with a typed plan origin and enforce dialogue authority at tool exposure, plan validation, persistence, and execution.
- Separate autonomous decision ownership from player dialogue with typed deterministic/LLM decision mode and off/LLM dialogue mode controls.
- Add bounded proxy pre-authentication admission control, handshake timeouts, a dedicated transparent world relay for unknown accounts, and startup collision checks.
- Replace the supervisor worker's unbounded transport with coalesced control state plus a bounded command queue.
- Add a durable per-bot JSONL action timeline so headless runs expose decisions, complete action lifecycles, revision context, durations, failures, and important state transitions without packet-log noise.
- Add typed hunter-pet happiness diagnostics, explicitly report that WotLK has no loyalty state, and publish pet transitions plus bounded periodic debug summaries.
- Expand deterministic AzerothCore provenance locking and verification to cover core/database revisions, modules, spell SQL, corrections, client build, and DBC hashes.
- Add semantic spell family roots and ability tags so combat policy does not depend on display-name matching.
- Split the largest safety-sensitive modules after behavior changes are complete while preserving public module paths.
- Add regression and saturation tests for authorization, proxy relay, queue behavior, provenance, and semantic tags.

## Capabilities

### New Capabilities

- `safety-boundaries`: Explicit encounter, dialogue, proxy, transport, and deterministic-data safety contracts.

### Modified Capabilities

None.

## Impact

- Snapshot schema and combat runtime state.
- Model/controller plan metadata and dialogue tool registry.
- Action validation and execution-time authorization.
- Grind, group, quest/goal, PvP, pet, interrupt, pursuit, target-switch, and AoE combat paths.
- Proxy configuration, auth/world listeners, transparent passthrough routing, and tests.
- Supervisor worker transport and worker lifecycle.
- Headless and interactive runtime observability.
- Spell provenance data, verification tooling, capability hydration, and combat selectors.
- Module layout only after behavior is covered by tests.
