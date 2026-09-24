# Local verification

Run the checks below after source or dependency changes. A successful build does not prove gameplay behavior against a live AzerothCore server.

Run these commands from the repository root:

```sh
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets
tools/check-binrw-future-compat.sh
```

The workspace pins Tentacli 15.3.2 in `Cargo.toml`. If Cargo reports an adapter API mismatch after a dependency change, compare the adapter with that pinned revision before changing the runtime interfaces.

The most important regression areas are:

- worker action validation rejects stale state, mission, permission, worker, and ownership generations;
- proxy transport validation rejects stale worker, ownership, and movement generations;
- player attach/movement/detach changes only the player-control pause reason;
- configured sessions remain the sole writers of their authoritative upstream WorldSession;
- transparent sessions do not acquire local bot authority;
- reducer state changes only from `ProtocolObservation` input;
- action audit entries produce one terminal result per action;
- protected paths and remote-memory configuration fail closed.

## Headless Warden live check

Set `proxy.warden_client_image` to a local WoW 3.3.5a `Wow.exe`. Enable Warden
on AzerothCore and start a headless account. The log must show `module
requested`, `module verified`, `hash verified`, `module initialized`, and
`checks answered`. Keep the account connected through several server check
rounds and confirm that the server does not kick it. An unknown module or
check must end the session with a clear error.

## Quest end-to-end smoke test

For the first autonomous quest test, place the configured character close enough to interact with an available quest giver, then run:

```text
.bot on
.bot quest
```

Expected diagnostics include, in order:

```text
mission installed in lane engine
quest scheduler requesting quest-giver statuses
validated gameplay action queued for proxy
bot gameplay packet transmitted            # opcode 0x417
quest scheduler opening authoritative quest giver
bot gameplay packet transmitted            # opcode 0x184
quest scheduler accepting authoritative quest offer
bot gameplay packet transmitted            # opcode 0x189
```

The proxy also sends in-game system notices for `.bot on`, `.bot off`, and mission changes.

This smoke path validates quest discovery/open/accept. Objective target selection and travel require additional authoritative object/objective projection; when that evidence is not available the lane must emit a `mission scheduler waiting` reason instead of silently idling.

## Audit-focused live checks

After `cargo check --workspace --all-targets` and `cargo test --workspace`, verify a configured login produces an `authoritative world-entry observation received` message before a non-idle mission begins acting.

For `.bot quest`, run near a quest giver and confirm the log progresses through server-derived observations rather than only repeating opcode `0x417`. If the scheduler waits, the reason must identify the missing authoritative state. Check active journal, objectives, navigation, completion, and turn-in before claiming an end-to-end quest result.


## Quest lifecycle live test

For a configured character with at least one active incomplete quest, run:

```text
.bot on
.bot quest
```

The runtime log should progress through the applicable stages below rather than stopping silently:

1. `mission installed in lane engine`
2. `quest scheduler requesting authoritative quest definition` followed by opcode `0x05C` / decimal `92` when the active quest definition is not hydrated yet.
3. An `authoritative quest observation` containing `QuestDefinition`.
4. One of:
   - `quest scheduler grounded creature objective from live object state`
   - `quest scheduler grounded game-object objective from live object state`
   - `quest scheduler starting bounded objective-area travel`
   - `quest item objective using AzerothCore loot-source search hint`
5. During travel, `quest movement progress` appears at debug level. If a live target enters visibility, `live authoritative quest target superseded search-area movement` should appear.
6. Out-of-range interactions become `validated quest action handed off to owned movement work`, then `resuming quest action after movement`.
7. Item objective progress is driven by `InventoryCount` observations derived from owned live item/container objects.
8. Completed quests progress through `complete-quest`, optional `request-reward`, and `choose-reward` steps, with each step waiting for authoritative server follow-up.

If the bot reaches a static search coordinate without a visible target, the expected state is a single `mission scheduler waiting` reason stating that it is waiting for a live authoritative target. It must not continuously restart the same movement.


## Quest item and special-control live checks

For an item collection quest, verify that the worker log changes the authoritative inventory count only after the server exposes the owned item/container update. The bot may use static loot-source coordinates to search, but it must not claim collection progress from static data.

For a game-object gather objective, verify this sequence when applicable:

```text
static search hint -> live game object observed -> UseGameObject -> authoritative inventory/quest counter update
```

For the Death Knight Eye of Acherus quest path, verify that the log shows the quest-bound control object or controlled mover becoming authoritative, the controlled action bar being observed, and an applicable controlled spell being selected for a live marker. It must not issue a normal `Attack` against the objective marker. Controlled movement should preserve the server-observed flying movement flags.

For the Power Converters quest (10584), verify the setup object is activated first, the lane waits until an authoritative Electromental is visible, then uses the observed Protovoltaic Magneto Collector item instance on that creature. With no live target, it must not send item use. After item use, it must wait for authoritative quest progress before repeating.

Unresolved quest-item-use candidates remain negative tests: the scheduler must not infer an item action from item possession or static spawn data alone. It needs a grounded quest-item rule, a matching incomplete objective, a live target, and an authoritative item instance.
## Deterministic reuse acceptance checks

The OpenSpec now treats duplicate deterministic mechanics as a quality defect. During review, verify that mission-specific code delegates to the canonical subsystem instead of maintaining parallel implementations. In particular:

- quest travel uses the shared movement/navigation runtime and shared movement packet construction;
- quest gathering/loot uses the shared gather/loot mechanics;
- quest inventory progress uses the shared authoritative inventory-counting helpers;
- quest live-target selection uses the shared deterministic target selector with typed filters;
- quest objective and item-source searches use one arrival range and shared search movement setup; turn-in search keeps its separate range;
- ordinary and controlled-unit casts use the shared action-validation and WotLK cast encoder with typed mover context;
- fixed-slot quest objective comparison, hint lookup, and turn-in dialog progression are reusable helpers used both before and after movement/replanning.

If equivalent behavior is implemented twice, treat the path as incomplete until the duplication is consolidated or the semantic difference is represented by an explicit typed policy/variant and tests.


### Regression: grounded target movement must not self-cancel

When a live quest target is already grounded but out of range, `NeedsMovement` creates `ApproachGroundedTarget` work. The live-target takeover rule applies only to static `SearchArea` movement. The approach movement must continue until the interaction envelope is reached or another real fence/preemption occurs.


## Quest loot regression
For bot-owned loot tests, start `.log start` before `.bot on` so the action capture contains bot traffic. Verify one CMSG_LOOT opens the window, followed by CMSG_AUTOSTORE_LOOT_ITEM for returned slots and CMSG_LOOT_RELEASE, with no repeated CMSG_LOOT each scheduler tick.

## Movement/scripted quest regression tests

After `cargo test --workspace`, live-test these paths with action logging started before automation:

```text
.log start
.bot on
.bot quest
```

For unrouted ground quest travel, verify `quest movement progress` Z values follow AzerothCore `maps/` terrain and the player does not climb through empty space. For routed travel, verify Z follows the Detour/MMAP route surface, including when raw terrain disagrees. For Lazy Peons (5441), verify the bot sends targeted item use rather than `Attack` and waits for quest credit. For Death Comes From On High (12641), verify the control object is activated, a controlled mover becomes authoritative, movement uses controlled flight, and Siphon/controlled ability credit advances the quest.

## Shared spatial-precondition regression checks

Targeted actions must use the common spatial sequence rather than mission-specific positioning logic:

```text
range -> server-backed LOS recovery -> facing -> action
```

Regression coverage must verify:

- an out-of-range attack/cast/interaction produces shared owned movement work;
- an in-range target outside facing tolerance emits `MSG_MOVE_SET_FACING` through the normal movement encoder;
- the matching mirrored bot-facing packet is suppressed as bot feedback and does not trigger player takeover;
- AzerothCore `SMSG_CAST_FAILED` reason 47 triggers deterministic LOS reposition through the shared movement controller;
- reasons 97 and 128 trigger shared move-closer / move-farther recovery;
- Lazy Peons, Eye of Acherus, combat, gathering, and future PvP callers reuse these mechanics rather than creating local range/facing/LOS implementations.

## Survival engagement and headless default regression checks

- NPC `UNIT_FIELD_TARGET` = player GUID: bot must defend even when mission did not pull it.
- NPC target = online group member: bot may defend through the same shared combat selector.
- Player entity target = bot without PvP authority: bot must not auto-attack.
- Survival attacker appears during quest movement: voluntary movement is fenced/stopped and survival response owns any required approach/facing.
- Survival target dies/despawns: survival movement/action ceases and prior mission replans from current authoritative state.
- Successful headless authoritative world entry: supervisor receives fresh Quest mission and bot ownership request without local chat commands.
- Pre-world-entry/Warden failure: no Quest work becomes runnable merely because an upstream socket exists.

## Idle-resume supervisor failure regression

The configured-session actor tests cover both outcomes after the player idle window:

- When the supervisor accepts the pause-clear request, the actor clears the idle-resume arm and publishes bot ownership.
- When the supervisor is unavailable, the actor keeps resume armed, returns ownership to the player, and does not publish bot ownership. Retry delay increases from one second to a 30-second cap.

The actor must not log a successful bot resume until the supervisor accepts the resume request.

## Multi-level MMAP vertical authority regression

For a ground route through a cave, bridge, building, tunnel, ramp, dungeon, or other stacked geometry:

1. Start a quest/travel action whose Detour route crosses a walkable layer whose Z disagrees with raw `maps/` terrain at the same X/Y.
2. Confirm route execution continues on the MMAP/Detour corridor instead of immediately failing with `FloorDiscontinuity` because of the unrelated terrain layer.
3. Confirm movement remains on the connected route surface through polygon boundaries; it must not snap to a floor above/below merely because that raw terrain height is numerically available.
4. Confirm ordinary unrouted ground movement still samples AzerothCore `maps/` terrain and still fails closed on an unsafe vertical discontinuity.
5. Confirm server-authorized flight still advances in explicit 3-D and is not terrain-clamped.

The cave turn-in regression that motivated this check previously selected a valid long MMAP route but failed its first execution step when raw terrain Z disagreed with the interior corridor. A successful run should show route progress rather than repeated identical turn-in work with immediate `FloorDiscontinuity`.
