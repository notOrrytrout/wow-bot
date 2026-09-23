## Why

The bot has useful group helpers, but group play is not a typed mission and has no shared encounter or formation model. As a result, operators cannot select party or raid roles, and the bot cannot reliably coordinate pulls, recovery, wipes, pets, marks, or instance transitions.

## What Changes

- Add typed `Party` and `Raid` bot missions with an explicit group role. `Auto` remains the default role.
- Add `.bot party [role]` and `.bot raid [role]` commands for `auto`, `tank`, `healer`, `melee`, `ranged`, and `support` roles.
- Add a shared, automatically refreshed group context for leaders, tanks, healers, members, pets, encounter units, marked targets, crowd-control targets, dead members, threat relationships, formation, and instance state.
- Add a deterministic group state machine for forming, following, pull waiting, engagement, role execution, recovery, regrouping, wipes, resurrection, raid preparation, boss phases, and resets.
- Preserve existing class combat rotations while a higher group policy selects targets, movement intent, recovery, and pull authorization.
- Make party behavior follow the leader, assist the assigned tank or leader, protect healers, avoid crowd control, limit chase distance, regroup, recover, resurrect, and handle instance entry and wipe recovery.
- Make raid behavior aware of main and off tanks, raid marks, threat limits, interrupt assignments, healer protection, role distance, stack or spread formation, observable hostile ground effects, boss phases, wipes, and resets.
- Expand encounter membership to include enemies that attack a member or pet, enemies attacked by the group, and assigned or marked enemies.
- Integrate group-aware loot decisions with the server-observed loot method and item roll state; do not bypass server group rules.

## Capabilities

### New Capabilities

- `group-missions`: Typed party and raid missions, command syntax, group context, encounter boundaries, state transitions, role tactics, instance awareness, and safe loot and recovery behavior.

### Modified Capabilities

None.

## Impact

- Mission serialization, persistence, descriptions, planner goals, model context, and mission gates.
- Proxy chat-command parsing and the authenticated proxy-to-supervisor mission API.
- Group-state parsing and enrichment, including raid metadata, marks, pets, threat observations, and map or instance classification.
- Party runtime policy, a new shared group mission runtime, combat target authorization, movement, death recovery, looting, and activity arbitration.
- Unit and integration tests across the bot and proxy crates. Existing stored mission formats remain readable.
