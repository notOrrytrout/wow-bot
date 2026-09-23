## 1. Typed Missions and Commands

- [x] 1.1 Add the serialized `GroupRole` type and `BotMission::Party` and `BotMission::Raid` variants, including stable kinds, planner goals, helpers, schema coverage, and role parsing; verify unit tests cover every role, command alias, default Auto behavior, and existing mission serialization.
- [x] 1.2 Extend proxy mission requests and `.bot party [role]` and `.bot raid [role]` parsing, validation, help text, and status responses; verify proxy tests accept all valid forms and reject unknown or extra arguments without sending a supervisor request.
- [x] 1.3 Extend the authenticated supervisor mission API, mission descriptions, persistence, memory mirror, and replacement flow for Party and Raid; verify round-trip tests and mission-revision tests pass for both new variants.
- [x] 1.4 Update exhaustive mission consumers in activity selection, collection and mission gates, supervisor feasibility, characterization, model context, and diagnostics; verify `cargo check --workspace` has no unhandled mission variants.
- [x] 1.5 Add compatibility fixtures for legacy quest, gather, grind, PvP, and goal mission stores; verify they deserialize with unchanged meanings after the enum expansion.

## 2. Group Observation Model

- [x] 2.1 Extend group state with an explicit known group type and authoritative raid metadata while preserving unknown values; verify raw party and raid group-list fixtures retain leader, subgroup, role, and group-type data.
- [x] 2.2 Add observed owner or summon relationships for player pets and guardians without counting them as group members; verify fixtures associate a Hunter pet with its owner and reject an unrelated creature as a pet.
- [x] 2.3 Parse and retain raid target icon assignments and clear them on authoritative removal or world reset; verify mark fixtures cover skull, X, reassignment, removal, and unknown state.
- [x] 2.4 Add the available attacker, victim, cast, threat-target, loot-method, roll-request, and hostile-area observations with explicit known or unknown state; verify each parser with protocol or trusted event fixtures and do not synthesize unavailable fields.
- [x] 2.5 Add trusted map classification for outdoor, dungeon, raid, battleground, arena, and unknown maps and bind it to observed map transitions; verify representative map fixtures and unknown IDs.

## 3. Shared Group Context and Encounter Model

- [x] 3.1 Add a pure derived `GroupContext` with group type, effective role, leader, tanks, healers, members, pets, marks, crowd control, dead members, threat relationships, formation, encounter members, and instance state; verify complete and partial-snapshot tests preserve unknown data.
- [x] 3.2 Resolve Auto role from authoritative assignment first and supported class, specialization, and equipment evidence second; verify unsupported explicit roles are reported and Auto never resolves to an unsupported role.
- [x] 3.3 Build encounter membership from attacks on the player, group members, and owned pets, plus group attacks, assist targets, and marks; verify the Hunter pet case is admitted while an unrelated nearby hostile is excluded.
- [x] 3.4 Add authoritative encounter removal and bounded stale expiry for death, evade, reset, despawn, and lost observations; verify stale targets do not survive a reset or cause a chase after the deadline.
- [x] 3.5 Centralize breakable crowd-control detection and expose protected targets in GroupContext; verify polymorph, sap, repentance, hex, shackle, and freezing-trap fixtures are excluded from ordinary target selection.

## 4. Group Runtime and State Machine

- [x] 4.1 Add the group runtime state, transition timestamps, encounter memory, pull time, recovery deadline, formation intent, interrupt assignment, and mission-revision reset behavior; verify a mission replacement clears all stale group runtime data.
- [x] 4.2 Implement pure Party lifecycle transitions for Forming, Following, WaitingForPull, Engaging, ExecutingRole, Recovering, Regrouping, Wiped, and Resurrecting; verify every legal transition and important rejected transition with table-driven tests.
- [x] 4.3 Extend the lifecycle for Raid Preparing, BossEncounter, PhaseTransition, and Resetting; verify boss engage, phase evidence, wipe, reset, and regroup transition tests.
- [x] 4.4 Expose current group state, configured role, resolved role, instance class, leader or anchor, encounter count, recovery hold, and formation intent in status and diagnostics; verify sanitized output contains no invented observations.
- [x] 4.5 Replace the old always-on party helper with the shared mission service for Party and Raid while preserving safe invite handling outside those missions; verify ordinary solo missions do not start voluntary group following or group pulls.

## 5. Party Movement, Pull, and Recovery Policy

- [x] 5.1 Add configurable role follow start, stop, combat range, spacing, and group leash values with safe defaults and validation; verify invalid or inverted envelopes fail configuration loading clearly.
- [x] 5.2 Select leader, assigned tank, or encounter centroid as a state-specific group anchor and emit one prioritized movement intent per tick; verify safety and death movement preempt formation movement.
- [x] 5.3 Implement leader following, map-transition regrouping, separation detection, wait-for-lagging-member behavior, and role spacing with full three-dimensional routed destinations; verify tests cover melee, ranged, healer, and missing-leader cases.
- [x] 5.4 Enforce non-tank pull discipline and the configurable tank threat-establishment delay; verify a leader target does not cause a pull before engagement and an admitted encounter target becomes attackable after the delay.
- [x] 5.5 Implement assist-target priority, healer and critical-member protection, crowd-control avoidance, and group-leash chase cancellation; verify deterministic target-selection tests cover all priority tiers and safe fallback behavior.
- [x] 5.6 Implement post-combat health and healer-mana recovery holds, nearby resurrection behavior, and recovery release conditions; verify no new voluntary pull or travel starts while recovery is active.
- [x] 5.7 Integrate group wipe detection with existing release, corpse navigation, instance-return, map-transition, regroup, and encounter-reset behavior; verify outdoor death, dungeon entrance return, partial wipe, full wipe, and recovered-group tests.

## 6. Role and Raid Tactics

- [x] 6.1 Feed effective role, target authorization, threat hold, movement envelope, and recovery intent into existing combat policy without selecting spells in the group layer; verify existing class rotation tests continue to pass with group constraints.
- [x] 6.2 Add main-tank, off-tank, assist, and raid-mark priority selection from observed assignments; verify explicit assist overrides skull, skull precedes X by default, and missing observations disable only that tactic.
- [x] 6.3 Add threat-sensitive damage authorization and existing threat-reduction ability integration; verify damage pauses or reduces threat when it becomes the hostile target before tank control.
- [x] 6.4 Add observed interrupt assignments and authorize existing class interrupts only for the assigned cast, range, and readiness; verify unassigned, unknown, late, and successful interrupt cases.
- [x] 6.5 Add Stack and Spread formation intents using reachable candidate points, known member positions, role spacing, hostile-envelope rejection, and the group leash; verify route rejection and no-safe-point behavior return to a safe wait state.
- [x] 6.6 Add hostile-area escape for effects with known ownership, location, envelope, and lifetime; verify observable hazards preempt formation and unknown or expired hazards do not move the bot.
- [x] 6.7 Add observed boss preparation, engagement, phase transition, and reset policy without generic boss-script guesses; verify unknown boss mechanics leave the bot in its safe role and formation policy.

## 7. Group Loot and Roll Safety

- [ ] 7.1 Add server-observed group loot method and live roll-request state to the loot decision path; verify no roll is emitted without a matching active request and eligibility.
- [ ] 7.2 Implement configurable pass, greed, disenchant, and need decisions using usable-upgrade evidence and operator limits; verify master-loot and need-before-greed fixtures cannot be bypassed.
- [ ] 7.3 Keep direct corpse and object looting behind existing ownership and lootability validation in group missions; verify party pet kills remain lootable only when the server authorizes this player or group.

## 8. Integration and Release Validation

- [x] 8.1 Add distance-aware Hunter attack selection that filters every ranged and melee ability by its observed range before applying the existing rotation priority; verify ranged-distance, melee-distance, dead-zone, cooldown, and no-usable-attack tests.

- [ ] 8.2 Add end-to-end snapshot fixtures for a five-player dungeon pull, Hunter pet aggro, healer recovery, party wipe, raid marked trash, raid boss phase, and instance return; verify expected state, target, and movement intents at each step.
- [ ] 8.3 Run `cargo fmt --all --check`, `cargo check --workspace`, all focused mission, proxy, group, combat, movement, death, loot, and Hunter tests, and `cargo test --workspace`; fix every failure before release validation.
- [ ] 8.4 Run `cargo build --release --workspace` and the bot doctor command with the repository configuration; record any environment-only warning separately and do not claim live dungeon or raid validation without an actual server test.
- [ ] 8.5 Document the new commands, roles, configuration values, diagnostics, known observation limits, Hunter range behavior, and a live party and raid test checklist; verify every documented command matches the parser tests.
