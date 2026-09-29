# Tasks

- [x] Select known food or drink from authoritative backpack instances only at the old low-health or low-mana trigger.
- [x] Require authoritative living, out-of-combat, stationary, uncontrolled player state.
- [x] Dispatch through `GameplayCommand::UseItemInstance` and hold a bounded lane-local lease while waiting for observed aura or resource progress.
- [x] Release at the old resource target; retry after stall, timeout, interrupted aura, combat, movement, or control takeover.
- [ ] Add typed stand-state observation and a validated sit/stand action before claiming full old recovery behavior.
- [ ] Verify food/drink recovery and combat preemption on a live server.
