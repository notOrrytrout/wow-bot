## Purpose

Define safe, typed party and raid missions that coordinate group movement, encounters, combat roles, recovery, instance transitions, and loot without replacing class combat rotations.

## ADDED Requirements

### Requirement: Typed group missions and roles
The system SHALL provide `party` and `raid` missions with the roles `auto`, `tank`, `healer`, `melee_dps`, `ranged_dps`, and `support`. The system SHALL default to `auto` and SHALL resolve `auto` only to a role supported by the character's observed class, specialization, equipment, and group assignment.

#### Scenario: Party mission uses the default role
- **WHEN** an operator selects party mode without a role
- **THEN** the stored mission is a party mission with the auto role

#### Scenario: Unsupported explicit role
- **WHEN** an operator selects a role the character cannot perform
- **THEN** the bot does not claim that role and reports the supported role choices

### Requirement: Party and raid commands
The proxy command interface SHALL accept `.bot party [role]` and `.bot raid [role]`, SHALL accept the short role names `melee` and `ranged`, and SHALL show both commands and their valid roles in `.bot help` and `.bot status` output.

#### Scenario: Select ranged raid behavior
- **WHEN** the operator sends `.bot raid ranged`
- **THEN** the supervisor stores a raid mission with the ranged DPS role and confirms it in chat

#### Scenario: Reject an invalid group role
- **WHEN** the operator sends `.bot party commander`
- **THEN** the proxy returns a usage message and does not replace the current mission

### Requirement: Automatic group and instance awareness
The system SHALL derive current group type, membership, leader, role assignments, map transition state, and whether the player is in an instanced map from authoritative observed game state. Party and raid missions SHALL adapt to instance state without a separate user-facing instance mission.

#### Scenario: Dungeon transition
- **WHEN** a party mission observes a transition from an outdoor map into a dungeon map
- **THEN** the mission remains active, refreshes its group context, and applies instance movement and encounter rules

#### Scenario: Raid-sized group
- **WHEN** a raid mission observes authoritative raid group metadata
- **THEN** the context preserves subgroup and raid-role information instead of treating the group as a five-player party

### Requirement: Shared group context
The system SHALL maintain a derived group context containing the group type, leader, tanks, healers, members, owned pets, active encounter units, marked targets, crowd-control targets, dead members, observed threat relationships, formation state, and instance state. Missing observations SHALL remain unknown and SHALL NOT be invented.

#### Scenario: Group member pet joins an encounter
- **WHEN** an enemy attacks a known party or raid member's pet
- **THEN** the enemy is included in the active group encounter

#### Scenario: Unknown raid mark data
- **WHEN** the client has not observed raid target icons
- **THEN** the context reports marks as unknown or empty and does not invent a marked target

### Requirement: Group-aware encounter boundary
The system SHALL include a living enemy in the active encounter when it attacks the player, attacks a group member, attacks a known group member pet, is attacked by the player or group, or is assigned by an observed raid mark or assist target. Encounter membership SHALL expire only after authoritative death, evade or reset evidence, or a bounded stale-observation timeout.

#### Scenario: Hunter pet is attacked
- **WHEN** a hostile creature attacks the tank's or another member's pet
- **THEN** group combat may defend the pet and the creature is not rejected as an unrelated pull

#### Scenario: Unrelated nearby enemy
- **WHEN** a hostile creature is only nearby and has no encounter relationship
- **THEN** a non-tank group bot does not independently pull it

### Requirement: Deterministic group lifecycle
Party missions SHALL use the states `forming`, `following`, `waiting_for_pull`, `engaging`, `executing_role`, `recovering`, `regrouping`, `wiped`, and `resurrecting`. Raid missions SHALL also use `preparing`, `boss_encounter`, `phase_transition`, and `resetting`. State changes SHALL use observed conditions and SHALL be visible in status or diagnostic output.

#### Scenario: Combat ends with low healer mana
- **WHEN** the encounter ends and the observed healer mana is below the configured recovery threshold
- **THEN** the mission enters recovery and does not start voluntary movement or a new pull

#### Scenario: Raid wipe
- **WHEN** no living observed raid member can continue the encounter and the group is dead or released
- **THEN** the raid mission enters wiped and then follows the reset and regroup flow

### Requirement: Leader following and regrouping
The system SHALL follow the group or raid leader with role-configurable start distance, stop distance, and formation spacing. It SHALL stop chasing an encounter target beyond the configured group leash and SHALL regroup when separated or after a map transition.

#### Scenario: Ranged role follows at ranged spacing
- **WHEN** a ranged DPS bot follows a visible leader outside combat
- **THEN** it follows to the configured ranged formation distance rather than melee distance

#### Scenario: Enemy leaves the group leash
- **WHEN** an enemy moves beyond the configured leash from the group anchor
- **THEN** the bot stops its voluntary chase and returns to the formation or leader

### Requirement: Pull and target discipline
Non-tank group roles SHALL wait for an observed tank or leader pull and a configurable threat-establishment interval before starting threat-producing attacks. Target selection SHALL prefer an assigned assist target and then observed raid marks in configured priority order. The system SHALL not attack an observed breakable crowd-control target unless that target is actively harming the group and no safe alternative exists.

#### Scenario: Tank has not pulled
- **WHEN** the leader targets an unengaged enemy and the bot is a non-tank
- **THEN** the bot waits and does not turn the target selection into a pull

#### Scenario: Skull and X are present
- **WHEN** both skull and X marked encounter targets are alive
- **THEN** the configured mark priority selects skull before X unless an explicit assist assignment overrides it

### Requirement: Role execution above existing rotations
The group mission policy SHALL select follow target, combat target, pull authorization, movement constraints, recovery intent, and effective role. Existing class rotation code SHALL continue to select the concrete spells and abilities within those constraints.

#### Scenario: Healer mission enters combat
- **WHEN** a healer-role party mission enters an encounter
- **THEN** group policy preserves healing priorities while the class rotation chooses available healing spells

#### Scenario: Threat-sensitive damage
- **WHEN** a damage-role bot observes that it is the hostile target before the tank has control
- **THEN** it reduces or pauses threat-producing damage and can use an available threat-reduction ability

### Requirement: Hunter range-appropriate attack selection
The Hunter combat policy SHALL select the highest-priority ready and usable ranged attack when the target is inside that attack's observed ranged envelope, and SHALL select the highest-priority ready and usable melee attack when the target is inside melee range. It SHALL NOT repeatedly select a ranged attack inside its minimum range or a melee attack outside melee range.

#### Scenario: Hunter has ranged distance
- **WHEN** a Hunter has an authorized target inside a ready ranged ability's observed minimum and maximum range
- **THEN** the rotation selects the highest-priority usable ranged attack instead of a melee attack

#### Scenario: Hunter is inside ranged dead zone
- **WHEN** a Hunter has an authorized target inside melee range and ranged attacks are below their observed minimum range
- **THEN** the rotation selects the highest-priority usable melee attack and does not retry the rejected ranged attack

### Requirement: Group protection and recovery
The system SHALL prioritize enemies attacking healers or critically injured members, SHALL resurrect nearby dead members when the role has a ready resurrection ability and combat is clear, and SHALL wait for observed health and healer mana recovery before resuming voluntary progress.

#### Scenario: Healer is attacked
- **WHEN** an active encounter enemy targets a known healer
- **THEN** protection target selection gives that enemy priority over a neutral assist fallback

#### Scenario: Resurrection is not available
- **WHEN** members are dead but the bot has no ready resurrection ability
- **THEN** the bot does not issue a false resurrection action and follows wipe recovery policy

### Requirement: Raid-specific tactics
Raid missions SHALL distinguish main tank and off-tank observations, support interrupt assignments, use role-specific distance rules, and track boss preparation, phase transition, reset, stack, and spread intents. The bot SHALL act only on tactics supported by observed targets, marks, auras, casts, positions, or other authoritative game state.

#### Scenario: Assigned interrupt
- **WHEN** a hostile boss cast matches the bot's active interrupt assignment and the interrupt is ready and in range
- **THEN** the role policy authorizes the class rotation to interrupt

#### Scenario: Spread intent with known member positions
- **WHEN** a spread intent is active and nearby raid member positions are known
- **THEN** movement selects a reachable position that meets configured role spacing without leaving the encounter leash

### Requirement: Observable hostile-area avoidance
The system SHALL avoid hostile ground effects only when their hostile ownership, location, radius or safe envelope, and active lifetime can be derived from observed game state or trusted encounter data. It SHALL not infer invisible hazards.

#### Scenario: Observable hostile area overlaps the bot
- **WHEN** an active hostile area with a known safe envelope overlaps the bot's position
- **THEN** safety movement preempts ordinary formation movement and selects a reachable point outside the area

### Requirement: Wipe and instance recovery
The system SHALL detect group wipes, stop encounter attacks, release only when release is safe and allowed, use existing corpse and instance-return navigation, wait for authoritative map transitions, regroup with living members, and reset encounter state before another pull.

#### Scenario: Corpse is outside an instance
- **WHEN** a released bot must cross an instance boundary to reach its corpse
- **THEN** recovery uses the known entrance transition and waits for the new map before continuing corpse navigation

#### Scenario: Encounter reset after wipe
- **WHEN** the group has recovered and the boss is no longer engaged
- **THEN** stale encounter targets and phase state are cleared before the mission returns to preparing or following

### Requirement: Group loot safety
The system SHALL follow the server-observed group loot method and roll state. It SHALL not send a loot or roll action that bypasses ownership, eligibility, master-loot, need-before-greed, or configured operator limits.

#### Scenario: Need-before-greed roll
- **WHEN** the server requests an eligible roll under need-before-greed
- **THEN** the bot selects only an allowed roll choice based on usable upgrade evidence and configured policy

#### Scenario: No observed roll request
- **WHEN** no server roll request exists
- **THEN** the bot does not manufacture a group roll action

### Requirement: Compatibility and mission replacement
Existing quest, gather, grind, PvP, and goal missions and existing persisted mission records SHALL remain readable. Replacing any mission with party or raid, or replacing party or raid with another mission, SHALL advance the mission revision and cancel stale queued or inferred voluntary work.

#### Scenario: Load an old mission store
- **WHEN** the supervisor loads a mission store created before group missions existed
- **THEN** all existing mission records deserialize with their prior meaning

#### Scenario: Replace raid with quest
- **WHEN** the operator selects quest mode while a raid decision or movement action is pending
- **THEN** the mission revision advances and stale raid work cannot execute
