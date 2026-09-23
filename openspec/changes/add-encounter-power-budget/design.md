## Context

The bot already has route-risk scoring and solo ranged/caster cluster safety. That safety was intentionally conservative but did not compare encounter power. The new work adds a narrow reusable calculation instead of changing movement geometry, quest behavior, or class rotations.

## Decisions

### Encounter power is separate from route risk

Route risk continues to estimate exposure along movement paths. Encounter power estimates whether the observed hostile pack is appropriate for the player. The survival layer consumes both.

### Missing gear score is neutral

The current state exposes equipment condition and known equipment count, but not a complete item-level gear score. The implementation therefore treats missing gear as neutral contribution with added uncertainty. It does not invent item scoring.

### Ranged/caster profiles stay more conservative

Solo ranged/caster profiles receive a modest risk multiplier and keep the kite-back behavior added in the prior change. Melee and tank profiles do not inherit ranged kite behavior.

## Non-Goals

- No movement geometry changes.
- No quest, provider, group, or raid changes.
- No new spell rotation engine.
- No invented item-level gear score.


### Recent death memory raises encounter risk

When a bot dies, the reducer records a bounded set of nearby hostile creature entries and positions that plausibly contributed to the death. MySQL memory persists that signal per bot. The encounter-power budget treats matching live creatures in the same map/location as more dangerous for a short window. This is an additive risk-memory signal only; it does not create permanent tombstones and it does not replace live hostile, level, rank, health, or route-risk evidence.


### Death-risk persistence uses cumulative semantics

The reducer owns mob-death count increments. Persistence treats the snapshot count as the authoritative cumulative count for that hazard cell. Durable and local memory therefore take the greater known count for the same bot, creature entry, map, and area cell instead of adding a cumulative snapshot again.

### Local and MySQL death-risk identity must match

The local fallback uses the same 80-yard area-cell identity as the MySQL `mob_death_risks` primary key. Deaths from the same creature entry on the same map but in different area cells remain independent hazards.

### Memory clear includes lethal mob memory

`clear_bot_memory` clears recent mob-death risks together with learned facts, failures, tool usage, spell usage, plans, conversations, and action history. A user-requested memory clear must not leave encounter-risk memory behind.

### Supervisor SIGTERM is a graceful shutdown signal

On Unix, the supervisor handles SIGTERM with the same worker-manager cancellation and proxy shutdown path used for Ctrl+C. Non-Unix platforms keep the existing Ctrl+C behavior.
