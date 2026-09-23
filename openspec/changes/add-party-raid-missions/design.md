## Context

See `proposal.md` for motivation. The project already has typed missions, mission revisions, authenticated proxy commands, group packet parsing, class rotations, activity arbitration, corpse recovery, movement routing, and a small always-on party helper. The current helper has no explicit role-bearing mission, no raid lifecycle, and no durable encounter or formation context. Observations are partial, so every group decision must preserve unknown state instead of treating absence as proof.

## Goals / Non-Goals

**Goals:**

- Put one deterministic group policy above existing class combat rotations.
- Reuse the existing mission revision, activity arbitration, navigation, combat, loot, and death systems.
- Make party and raid behavior testable without a live server by deriving decisions from snapshots.
- Keep player commands small and stable while instance awareness stays automatic.

**Non-Goals:**

- A separate `.bot instance` command or a third instance mission.
- Scripted support for every boss encounter in the first implementation.
- Guessing invisible hazards, threat values, marks, roles, or phase changes.
- A second spell rotation engine or direct changes to server group rules.

## Decisions

### Use role-bearing mission variants

Add `BotMission::Party { role: GroupRole }` and `BotMission::Raid { role: GroupRole }`. `GroupRole` is serialized in snake case and provides `Auto`, `Tank`, `Healer`, `MeleeDps`, `RangedDps`, and `Support`. A shared parser accepts command aliases while serialization stays canonical.

This keeps operator intent durable and uses the existing mission revision to invalidate stale work. An untyped flag inside the party runtime was rejected because it would not survive supervisor persistence and would not be visible to mission gates.

### Derive one snapshot-level GroupContext

Add a pure `GroupContext::derive(snapshot, mission, observations)` step. It normalizes group type, effective role, leader, tanks, healers, pets, marks, crowd control, encounter members, dead members, threat relationships, formation, and instance classification. Fields whose source has not been observed use `Option` or an explicit unknown enum.

The context is derived rather than persisted. This prevents stale GUID, target, map, and phase data from becoming durable. The runtime stores only transition memory such as the current group state, encounter timestamps, pull time, formation intent, interrupt assignment, and bounded stale deadlines.

### Replace the party helper with a shared state machine service

The existing party service becomes the entry point for party and raid missions. It runs before voluntary mission planning and requests `ActivityKind::Party`. Death, urgent combat defense, hostile-area escape, and required loot retain higher priority.

Party states are `Forming`, `Following`, `WaitingForPull`, `Engaging`, `ExecutingRole`, `Recovering`, `Regrouping`, `Wiped`, and `Resurrecting`. Raid adds `Preparing`, `BossEncounter`, `PhaseTransition`, and `Resetting`. Transition functions are pure where possible and return an intent; the executor converts the intent into existing actions.

A set of unrelated booleans was rejected because it permits invalid combinations such as recovering, chasing, and preparing at the same time.

### Keep group intent separate from class action choice

The group layer produces constraints and intent: effective role, authorized encounter targets, preferred target, group anchor, movement envelope, threat hold, interrupt authorization, recovery hold, and formation intent. Existing combat policies remain responsible for exact spells.

This design avoids duplicated class logic and lets current healer and threat-aware policies consume better context. Direct spell selection in the group service was rejected because it would conflict with class cooldown, resource, and aura logic.

### Build encounter membership from observed relationships

Encounter membership is the union of enemies that attack the player, a member, or an owned pet; enemies attacked by the player or a member when attacker data is observable; marked or assigned targets; and previously admitted living enemies within the stale deadline. Authoritative death, evade or reset evidence removes a member immediately.

Pet ownership must be indexed from observed owner or summon data. A pet is part of its owner's encounter, but it is not a separate group member for role or wipe counts.

### Use anchors, envelopes, and reachable points for movement

The leader, assigned tank, or encounter centroid becomes the group anchor by state. Each role gets configurable follow start, follow stop, combat minimum and maximum range, leash, and member spacing. Movement requests use the current map and full three-dimensional destination through the existing route service.

Stack and spread are formation intents, not absolute coordinates. The movement layer evaluates nearby candidates, rejects hostile envelopes and unreachable points, and stays inside the group leash. This reuses navigation and prevents group logic from issuing raw forward movement.

### Treat raid features as observed assignments

Main tank, off tank, assist target, marks, interrupt assignments, boss identity, phase, stack, and spread data can come from authoritative packets, trusted configuration, or deterministic observation. Missing data disables only that tactic. The bot does not invent a boss script.

Raid mode is not implemented as leader-follow plus leader-target because that fails on multi-tank, interrupt, movement, and phase mechanics.

### Integrate commands through the current authenticated channel

Extend the proxy and supervisor mission request enums with Party and Raid variants that carry a role. Parsing is case-insensitive, accepts no argument as Auto, maps `melee` and `ranged` to canonical DPS roles, and rejects extra or unknown arguments before the request reaches the supervisor.

### Extend state only from protocol evidence

Enrich current group and entity observations for group type, raid flags, raid target icons, owner GUIDs, combat relationships, casts, dynamic hostile areas, and loot rolls only where the WotLK protocol or the existing event source exposes them. Instance classification uses trusted map metadata and observed map transitions. The state schema records whether each category is known.

### Apply group loot policy at the action boundary

Group policy can recommend loot and roll choices, but the existing action validator remains the final authority. Roll actions require a live server roll request, eligibility, and an allowed policy result. Direct loot keeps current ownership and lootability checks.

## Risks / Trade-offs

- [Some server observations do not contain full threat, pet ownership, or hazard data] → Preserve unknown values and enable only tactics supported by evidence.
- [A full raid framework is a large cross-cutting change] → Implement it in vertical slices: mission and commands, context, party state machine, encounter model, recovery, then raid tactics.
- [Formation movement can fight combat range movement] → Produce one prioritized movement intent per tick and let safety, death, and forced combat preemption win.
- [Stale encounter membership can cause chase or target errors] → Use authoritative removals plus short, state-specific stale deadlines and the group leash.
- [Automatic role inference can be wrong with incomplete observations] → Restrict Auto to observed supported roles, prefer authoritative group role assignments, and expose the resolved role in diagnostics.
- [Old mission files could fail after enum changes] → Keep existing tags unchanged, add only new variants, and add old-format fixture tests.

## Migration Plan

1. Add the role type and mission variants without changing the default mission or existing serialized tags.
2. Extend command and supervisor APIs, persistence tests, status text, model context, and mission gates.
3. Add group context and state-machine diagnostics behind Party and Raid missions.
4. Move existing party helper behavior into the shared service and add encounter, formation, recovery, and pet rules.
5. Add raid-only assignments and tactics when the required observations are known.
6. Run formatting, checks, focused tests, the full workspace test suite, and a release build.

Rollback consists of selecting an existing mission before installing the prior binary. Persisted Party or Raid records must be changed to an older mission type before a binary that does not know the new variants reads the mission store.
