## Why

`0013.6` still has mode-state failures that can stop safe combat handling, interrupt fishing between asynchronous phases, cast fishing in the wrong place, misclassify fish resources, wander on impossible gather work, and expose collection actions that the executor later rejects. These defects cross execution, activity ownership, world knowledge, supervisor status, and model-facing tool gates, so they need one coordinated reliability change.

## What Changes

- Replace collection-complete early return with a completion-drain flow that keeps forced combat, death, recovery, and loot active until logout is safe.
- Preserve the Fishing activity lease while the fishing state machine is active, including `WaitBobber` and `UseBobber`.
- Add deterministic travel to trusted fishing locations before casting, without treating fishing holes as normal clickable gather nodes.
- Classify authoritative fishing item names as Fishing resources instead of relying only on gather-node names.
- Detect Mining, Herbalism, and Fishing work that cannot be performed at the current profession rank and stop unproductive roaming.
- Give collection mode a top-level tool gate that takes precedence over the underlying Quest, Gather, Grind, or Goal mission gate.
- Keep the configured collection quest ID private while using it to remove unrelated quest offers from model-facing choices.
- Add regression tests for every affected transition and release gate before labeling the implementation `0013.7`.

## Capabilities

### New Capabilities

- `bot-mode-flow-reliability`: Defines safe completion draining, fishing lifecycle ownership and navigation, gather feasibility checks, and collection-specific decision boundaries.

### Modified Capabilities

- None.

## Impact

Primary code areas are expected to include `src/plugin.rs`, `src/plugin/harness.rs`, `src/plugin/fishing.rs`, `src/plugin/gather.rs`, `src/plugin/action_executor.rs`, `src/world_semantics.rs`, `src/world_knowledge.rs`, `src/supervisor.rs`, `src/collection.rs`, `src/llm/tools.rs`, related LLM choice projection code, and world-knowledge generation under `tools/`. The bundled world-knowledge data format may need an additive fishing-item index. Existing collection configuration syntax and public model-visible schemas must remain compatible; private target IDs must stay out of serialized model context.
