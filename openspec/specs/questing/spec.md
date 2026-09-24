# Questing Specification

## Purpose

Define authoritative quest lifecycle behavior, quest bootstrap, trusted routing hints, bounded quest work, and correct turn-in progression.

## Requirements

### Requirement: Authoritative quest lifecycle
The system SHALL base quest acceptance, active progress, completion, and turn-in eligibility on authoritative quest state.

#### Scenario: Quest hint exists without active journal entry
- **WHEN** static knowledge contains a quest hint but authoritative quest state does not show the quest as active
- **THEN** the quest is not treated as active work

### Requirement: Actionable quest-source inspection
The system SHALL prefer currently observed actionable quest sources when selecting deterministic local quest interactions.

#### Scenario: Visible NPC advertises an available quest
- **WHEN** an observed NPC is proven to offer an eligible quest
- **THEN** questing may inspect or accept that quest before aimless search behavior

### Requirement: Bounded quest bootstrap
The system SHALL permit a wider trusted starter-search horizon when no active quest work exists and SHALL contract to normal bounded work selection after active quest work exists.

#### Scenario: Quest journal is empty
- **WHEN** no active quest work exists
- **THEN** the runtime may use trusted starter hints within its bootstrap policy

### Requirement: Quest work uses scoped child tasks
The system SHALL perform travel, combat, gathering, and interaction needed for a quest as scoped work under the quest rather than replacing mission identity.

#### Scenario: Quest requires gathering
- **WHEN** an active quest objective requires gathering
- **THEN** gather work executes as quest-scoped child work
- **AND** the top-level quest mission remains active

### Requirement: Authoritative completion before turn-in
The system SHALL not treat an attempted objective action as proof of quest completion.

#### Scenario: Objective interaction was attempted
- **WHEN** an interaction occurs but authoritative quest state has not advanced to completion
- **THEN** the runtime does not proceed as if the quest is completed

### Requirement: Quest command drives the authoritative gameplay loop
A runnable quest mission SHALL be connected end-to-end from proxy command routing through worker mission evaluation, authoritative server observation, final action validation, proxy transport fencing, and WotLK quest packet encoding. Installing a quest mission without scheduling or observing quest work SHALL NOT satisfy this requirement.

#### Scenario: Available quest giver is observed
- **GIVEN** `.bot quest` is active and bot ownership is committed
- **WHEN** AzerothCore reports an available quest giver
- **THEN** the worker projects that server evidence into authoritative quest state
- **AND** the quest scheduler may open the authoritative giver through the validated action path
- **AND** the proxy encodes the corresponding WotLK quest-giver interaction only after transport authority passes

#### Scenario: Quest list is returned
- **WHEN** AzerothCore returns a quest-giver quest list
- **THEN** the returned giver and quest identifiers become authoritative quest-offer evidence
- **AND** an eligible offer may be accepted through a typed quest action
- **AND** repeated accept attempts are bounded while awaiting server evidence

#### Scenario: Quest giver returns an empty list
- **WHEN** the server returns a valid quest-giver list with zero offers
- **THEN** the response is recorded for that live giver
- **AND** the scheduler waits before it opens that giver again

### Requirement: Static quest knowledge is combined with live evidence
Static AzerothCore-derived quest knowledge MAY nominate likely quest sources, objective areas, or interaction targets, but the runtime SHALL combine those hints with live authoritative object/quest evidence before committing an interaction or claiming current presence.

#### Scenario: Static hint identifies a quest giver location
- **GIVEN** generated static knowledge names a likely quest giver and location
- **WHEN** no current authoritative object observation proves that giver is present
- **THEN** the hint may guide bounded search or travel
- **AND** it does not by itself authorize a quest interaction

### Requirement: Active quest objectives are hydrated before execution
For every active incomplete quest, the runtime SHALL obtain an authoritative objective definition from the connected AzerothCore session when the definition is not already known. The definition SHALL preserve the original fixed objective-slot indices so live quest-log counters are compared with the correct required counts.

#### Scenario: Active quest has no hydrated definition
- **GIVEN** the authoritative player quest journal contains an incomplete quest
- **WHEN** the worker has no definition for that quest
- **THEN** it requests the quest definition from AzerothCore through the validated action path
- **AND** it does not invent target IDs or required counts from a static hint

### Requirement: Quest objective resolution prefers live targets over search hints
The quest scheduler SHALL resolve each incomplete objective in this order: live authoritative target evidence, server-provided quest POI/search guidance, generated AzerothCore static spawn guidance, then an explicit unresolved state. Static guidance SHALL never by itself authorize combat, gathering, looting, or interaction.

#### Scenario: Static and live target both exist
- **GIVEN** a static spawn hint exists for an objective entry
- **AND** a live authoritative entity with that entry is observed
- **WHEN** the objective is selected
- **THEN** the live entity GUID and position are used for action validation
- **AND** the static coordinate is ignored for interaction authority

### Requirement: Quest objective movement resumes the originating work
When an otherwise valid quest action requires movement, the scheduler SHALL create quest-scoped movement work that retains a typed resume command. After the canonical movement state enters the action envelope, the runtime SHALL stop movement and revalidate the resume command before transmission.

#### Scenario: Quest target is out of interaction range
- **WHEN** final validation returns `NeedsMovement` for a quest interaction or attack
- **THEN** movement becomes the active child work
- **AND** the quest mission and semantic quest work identity remain active
- **AND** the original action is re-proposed only after the movement envelope is reached

### Requirement: Item objectives use authoritative inventory counts
Quest item-objective progress SHALL use packet-derived inventory ownership and stack counts for the active character. Static loot tables MAY identify likely sources but SHALL NOT be treated as proof that an item was collected.

#### Scenario: Quest item is looted
- **WHEN** an owned item/container object update changes the authoritative count for a required quest item
- **THEN** the item objective is reevaluated from that count
- **AND** the scheduler does not infer collection merely from a loot or gather attempt

### Requirement: Static search arrival waits for live grounding
When quest work reaches a server POI or generated static spawn/search hint and no matching live authoritative entity is observed, the runtime SHALL remain in bounded search/wait state rather than treating the hint as an interaction target or repeatedly restarting the same movement.

#### Scenario: Search coordinate reached without target
- **WHEN** the character enters the configured search envelope around a static objective hint
- **AND** no authoritative matching entity is visible
- **THEN** movement stops
- **AND** the quest work reports that it is waiting for live target evidence

#### Scenario: Search area has multiple static spawn hints
- **GIVEN** no matching live authoritative entity is visible at the nearest hint
- **WHEN** the character reaches that hint
- **THEN** the scheduler tries the next nearby distinct hint, up to its bounded candidate limit
- **AND** it waits before repeating the same candidate set when no candidate has a live target

### Requirement: Quest object use waits for progress evidence
The scheduler SHALL wait for authoritative objective progress, required inventory progress, a controlled-mover change, or target disappearance after a quest object-use action. An unresolved use SHALL receive an increasing bounded retry delay.

#### Scenario: Quest game object gives no progress
- **WHEN** the server does not report objective or item progress after a game-object use
- **THEN** the scheduler does not use that object again immediately
- **AND** it logs the baseline and current authoritative progress before a delayed retry

### Requirement: Quest turn-in follows authoritative reward dialogs
Quest completion SHALL follow AzerothCore's authoritative turn-in dialog progression. `CMSG_QUESTGIVER_COMPLETE_QUEST` alone SHALL NOT be treated as successful reward completion.

#### Scenario: Required-items dialog is returned
- **WHEN** AzerothCore returns `SMSG_QUESTGIVER_REQUEST_ITEMS`
- **THEN** the dialog becomes authoritative turn-in state
- **AND** the worker requests the reward only when the server reports that completion is allowed

#### Scenario: Reward offer is returned
- **WHEN** AzerothCore returns `SMSG_QUESTGIVER_OFFER_REWARD`
- **THEN** the reward offer becomes authoritative turn-in state
- **AND** the worker submits a typed reward-choice action through normal validation and transport fencing
- **AND** the quest is not considered removed/completed until the authoritative quest journal changes

### Requirement: Quest protocol retries are bounded by server evidence
Quest accept and turn-in protocol steps SHALL wait for authoritative follow-up and SHALL NOT be transmitted continuously while the previous step is unresolved.

#### Scenario: Reward request was transmitted
- **WHEN** no corresponding server dialog or journal change has arrived
- **THEN** the runtime waits for a bounded confirmation interval before retrying
- **AND** it emits the current waiting reason to diagnostics

### Requirement: Quest-bound control objects and controlled movers remain authoritative
A quest MAY require activating a quest-bound game object that transfers movement or ability control to another unit. The runtime SHALL discover such control objects from trusted AzerothCore static metadata only as search guidance, SHALL require a live authoritative game-object observation before activation, and SHALL treat the server-observed controlled mover as the canonical mover until control is released.

#### Scenario: Quest uses a control mechanism
- **GIVEN** an active quest is associated with an AzerothCore quest-bound goober/control object
- **WHEN** the control object is not currently observed
- **THEN** its static spawn may guide bounded travel/search
- **AND** the bot does not fabricate activation
- **WHEN** the live game object is observed
- **THEN** activation may proceed through normal final validation and transport fencing

#### Scenario: Server grants controlled-unit movement
- **WHEN** authoritative object state shows the player now charms/controls another mover
- **THEN** movement decisions use that mover's authoritative position and movement flags
- **AND** the player body's position is not used as the controlled mover position
- **AND** release of the controlled mover returns movement authority to the player body

### Requirement: Controlled quest abilities are observed, not assumed
For quest work performed through a controlled unit or vehicle, the runtime SHALL use abilities currently exposed by authoritative server action-bar state. Static AzerothCore spell/target conditions MAY identify which observed ability is relevant to an objective, but the runtime SHALL NOT cast an ability that is absent from the current controlled-unit action bar.

#### Scenario: Eye of Acherus objective is grounded
- **GIVEN** the active quest objective targets an observed analysis marker
- **AND** the player controls the Eye of Acherus
- **AND** the server-exposed controlled-unit action bar contains Siphon of Acherus
- **WHEN** the objective is selected
- **THEN** the runtime uses the observed controlled-unit spell on the live objective target
- **AND** it does not reinterpret the marker as an ordinary kill objective

### Requirement: Quest item collection and quest-item use are distinct
The runtime SHALL distinguish collecting required quest items from using a quest/source item as an action. Inventory counts prove collection progress only. A quest that requires using an item or item-provided spell SHALL require separate authoritative action semantics before it is considered supported.

#### Scenario: Required item count increases
- **WHEN** authoritative inventory state reaches the required item count
- **THEN** the collection objective is satisfied from that count
- **AND** no additional use-item action is inferred solely from possession

#### Scenario: Quest requires using a source item
- **WHEN** the quest requires an item-provided action rather than mere collection
- **AND** the runtime has not grounded the required use semantics
- **THEN** it reports the objective as unsupported/waiting instead of treating possession as completion

### Requirement: Quest objectives resolve to typed execution semantics
Each incomplete quest objective SHALL resolve to an explicit typed execution semantic before mechanical execution. Supported semantics SHALL include at least kill/combat, gather-or-loot, game-object interaction, controlled-unit ability, item-use when grounded, and unsupported/waiting. The scheduler SHALL NOT infer mechanical behavior solely from the presence of a creature, game-object, or item identifier.

#### Scenario: Creature objective is actually a controlled-ability target
- **WHEN** AzerothCore quest metadata and authoritative controlled-unit state show that a creature objective is progressed by an observed controlled ability
- **THEN** the objective resolves to controlled-unit ability work
- **AND** it does not fall through to generic attack work

#### Scenario: Item objective is collection only
- **WHEN** authoritative objective definition and inventory state indicate an item-count objective without grounded item-use semantics
- **THEN** the objective resolves to collection/gather-or-loot work
- **AND** the scheduler does not fabricate a use-item action

### Requirement: Quest execution reuses shared deterministic subsystems
Questing SHALL own quest lifecycle, objective selection, semantic quest-work identity, and quest-specific policy, but SHALL delegate mechanical execution to shared deterministic subsystems whenever equivalent mechanics already exist. Quest code SHALL NOT duplicate movement, combat, gathering, looting, inventory counting, target selection, interaction validation, spell casting, or WotLK packet construction.

#### Scenario: Quest objective requires gathering
- **WHEN** an incomplete quest objective resolves to gather-or-loot work
- **THEN** the quest scheduler creates quest-scoped gather work using the shared gathering/loot runtime
- **AND** progress remains tied to the quest work identity and authoritative quest/inventory evidence

#### Scenario: Quest objective requires controlled spell casting
- **WHEN** a controlled-unit quest objective resolves to a targeted spell action
- **THEN** questing uses the shared action-validation and spell-encoding path with the controlled mover as typed context
- **AND** questing does not maintain a second quest-only cast encoder

### Requirement: Common quest resolvers are deterministic and reusable
Quest definition hydration, fixed-slot objective progress comparison, authoritative inventory counting, nearest-live-target selection, static-search-hint lookup, and turn-in dialog progression SHALL be exposed as deterministic reusable operations rather than duplicated across quest phases. Equivalent inputs SHALL produce equivalent results independent of which quest-work phase invoked them.

#### Scenario: Objective is replanned after movement
- **WHEN** a quest objective is reevaluated after movement or server state change
- **THEN** the same objective resolver used for initial planning is invoked again
- **AND** the runtime does not use a separate ad hoc post-movement resolution branch


### Requirement: Quest semantic actions wait for authoritative completion
After quest work emits a state-changing semantic action such as combat start, loot, interaction, or controlled-unit cast, the quest scheduler SHALL wait for the action's authoritative completion/progress evidence or a bounded timeout before emitting an equivalent retry. A scheduler tick SHALL NOT itself be treated as permission to repeat the same action.

#### Scenario: Quest item source is attacked and then looted
- **WHEN** quest-scoped work starts combat against a grounded item source
- **THEN** the scheduler waits for authoritative death/despawn evidence before transitioning to loot
- **AND** after loot is initiated, it waits for authoritative inventory, loot-release, or despawn evidence before another loot attempt

### Requirement: Scripted targeted quest-item use waits for authoritative quest credit
When a quest objective requires using an item-provided spell on a live target, questing SHALL resolve that objective to the shared targeted item-use mechanic using the authoritative item GUID and backpack slot. After the action is sent, the scheduler SHALL gate retries on authoritative quest-objective progress rather than target presence or a fixed short delay.

#### Scenario: Lazy Peons objective
- **WHEN** quest 5441 is active, a living Lazy Peon is authoritative, and the Foreman's Blackjack item instance is authoritative
- **THEN** questing uses the shared targeted item-use path instead of combat
- **AND** out-of-range use transitions through shared movement work
- **AND** after use the scheduler waits for the authoritative objective counter to advance before selecting another peon or retrying

### Requirement: Quest control-object activation waits for controlled-mover authority
When a scripted quest control object requires a spell-targeted activation transaction, questing SHALL use the shared game-object spell action and SHALL emit any required game-object report-use frame through the proxy encoder. After activation, the scheduler SHALL wait for authoritative controlled-mover state before proceeding with controlled movement or abilities.

#### Scenario: Death Comes From On High control object
- **WHEN** quest 12641 is active and live game object 191609 is actionable
- **THEN** the runtime activates it through the typed game-object spell path using the known activation spell
- **AND** it does not repeatedly activate the object while waiting for the server to expose the controlled Eye mover
- **AND** once the controlled mover is authoritative, objective travel and casts use shared controlled-mover movement and action mechanics

### Requirement: Bot combat transitions through corpse loot before new quest targeting
When authoritative combat completion leaves a lootable corpse entity present, quest execution SHALL create bounded post-combat loot work before selecting another quest target. This applies even when the quest objective itself is a kill counter rather than an item collection objective.

#### Scenario: Quest kill target dies and corpse remains
- **WHEN** authoritative entity health reaches zero for the current bot combat target
- **AND** the corpse entity remains authoritative
- **THEN** the runtime issues loot through the shared loot action path before selecting the next objective target
- **AND** loot completion is confirmed by authoritative loot transaction state rather than by packet transmission alone

#### Scenario: Corpse disappears before loot can occur
- **WHEN** the completed combat target is no longer authoritative before post-combat loot begins
- **THEN** the scheduler does not fabricate loot completion
- **AND** it continues quest planning without blocking indefinitely
