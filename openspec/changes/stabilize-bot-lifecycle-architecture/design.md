## Context

Movement and action lifecycle policy is distributed. Server-controlled movement and pending route planning must not create replacement semantic actions.

## Decisions

- Treat `AwaitingRoute`, `Moving`, `Suspended`, and tick-owned movement as active logical movement.
- Replans replace waypoints, not action or task identity.
- Retain quest travel until stand-off or terminal closure.
- Use only `Completed`, `Failed`, `TimedOut`, and `Interrupted` terminal statuses.
- Bound retries and diagnostics by stable scope, release conditions, and state changes.
- Preserve recoverable work during forced-activity preemption.
- Keep strict timeline durability as the baseline and defer event-driven fleet scheduling until gameplay stability.

## Non-Goals

No route-geometry rewrite, movement-speed change, broad revert, or wholesale runtime rewrite.
