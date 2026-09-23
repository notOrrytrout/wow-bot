## Why

Lifecycle ownership is distributed across movement, quest, combat, recovery, and runtime code. This causes logical actions to restart during server movement, route replanning, or temporary planner delay.

## What Changes

- Preserve one logical movement owner across awaiting, moving, suspended, and tick-owned phases.
- Preserve quest travel identity until stand-off or terminal closure.
- Centralize terminal and retry semantics, bounded diagnostics, durable timelines, and future event-driven scheduling.

## Impact

This specification does not change runtime behavior, route geometry, or movement speed.
