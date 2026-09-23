# Gathering and Professions Specification

## Purpose

Define resource-name scoping, node legality, fishing ownership, profession capability, training, crafting, and bounded material acquisition.

## Requirements

### Requirement: Gather target scoping
The system SHALL limit voluntary gathering to resources that satisfy the active gather mission's configured matching rules.

#### Scenario: Similar but different resource is observed
- **WHEN** a node's normalized name does not equal the configured resource under the mission's matching rule
- **THEN** the node is not selected as a voluntary gather target

### Requirement: Gather capability and skill validation
The system SHALL verify applicable profession capability and known skill requirements before committing to a gather node.

#### Scenario: Node exceeds known skill
- **WHEN** authoritative profession state proves the node requires a higher skill than the bot has
- **THEN** the node is rejected for gathering

### Requirement: Authoritative gather completion
The system SHALL require authoritative interaction and loot outcomes before considering a gather operation complete.

#### Scenario: Node interaction does not produce confirmed loot
- **WHEN** object use is attempted but authoritative state does not confirm the expected gather result
- **THEN** the system does not record successful gathering solely from the attempt

### Requirement: Fishing cast ownership
The system SHALL accept a fishing bobber only when it belongs to the current fishing attempt and to the player.

#### Scenario: Old or foreign bobber is observed
- **WHEN** a bobber predates the current cast or belongs to another player
- **THEN** it does not satisfy the current fishing attempt

### Requirement: Fishing remains distinct from ordinary object gathering
The system SHALL not treat fishing holes and fishing bobbers as ordinary chest-style gather objects when fishing-specific state is required.

#### Scenario: Fishing hole is selected
- **WHEN** the selected resource requires fishing mechanics
- **THEN** the fishing workflow verifies fishing capability, current cast, owned bobber, readiness, and authoritative loot completion

### Requirement: Profession capability is authoritative
The system SHALL derive learned profession capability and skill from authoritative state or another reviewed authoritative capability source.

#### Scenario: Static knowledge names a trainer
- **WHEN** static knowledge suggests a trainer but current trusted knowledge does not establish an actionable trainer interaction
- **THEN** the system may route toward the hint under policy but cannot complete training without a valid trainer state

### Requirement: Profession bootstrap preserves learned primaries
The system SHALL preserve already learned primary professions while applying configured/default profession bootstrap policy.

#### Scenario: Bot already has learned primary professions
- **WHEN** profession bootstrap runs
- **THEN** it does not replace those learned primary professions merely to satisfy a default recommendation

### Requirement: Crafting validates recipe and materials
The system SHALL verify recipe knowledge, skill, and required materials before starting an automatic craft.

#### Scenario: Reagent is missing
- **WHEN** a selected known recipe lacks required material quantity
- **THEN** the craft does not execute
- **AND** any material-supply work remains bounded and scheduler-controlled

### Requirement: Default cooking bootstrap
The system SHALL treat Cooking as a required target of the documented default profession bootstrap policy when that policy is active and the capability can be acquired legally.

#### Scenario: Cooking is not learned under default bootstrap
- **WHEN** default profession bootstrap is active and a legal bounded path to Cooking is available
- **THEN** the runtime may schedule Cooking acquisition without replacing already learned primary professions

### Requirement: Optional First Aid cannot deadlock progression
The system SHALL not permanently block ordinary progression solely because optional First Aid acquisition is currently unavailable.

#### Scenario: First Aid trainer is unavailable
- **WHEN** First Aid is desirable under policy but no valid trainable path is currently available
- **THEN** ordinary mission progression remains eligible to continue

