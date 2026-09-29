## ADDED Requirements

### Requirement: Class training may use recent local trainer observations
When authoritative player level state initializes or increases, class training SHALL become due. The lane MAY travel to a remembered class trainer only when that trainer position came from an observed class-trainer entity within the previous six hours, is on the current map, and is within 30 yards of the player's current position. It SHALL preserve the active mission intent and require both maintenance and movement permission, solo group state, no authoritative combat, more than 1,000 copper, and no observed quest offer within 40 yards. It SHALL cancel the trip when a gate becomes false or a live class trainer comes within interaction range. A failed visit or eligible but unaffordable offer SHALL delay another travel attempt for at least five minutes. A class trainer list with no eligible unknown class spell SHALL clear the training need.

#### Scenario: A recent same-map class trainer is a short detour
- **WHEN** training is due and a trainer was observed on the current map less than six hours ago
- **AND** the trainer location is within 30 yards of the player
- **AND** every travel gate is clear
- **THEN** the lane may route to that location
- **AND** the active mission intent remains unchanged

#### Scenario: Trainer is stale, distant, or on another map
- **WHEN** the remembered trainer observation is older than six hours, is more than 30 yards away, or is on a different map
- **THEN** the lane does not route to it

#### Scenario: Higher-priority work or missing permission blocks service travel
- **WHEN** the player is in combat, grouped, has no more than 1,000 copper, lacks maintenance or movement permission, or has an observed quest offer within 40 yards
- **THEN** the lane does not start or continue the trainer detour

#### Scenario: Trainer cannot be confirmed at the remembered location
- **WHEN** the lane reaches the remembered point but no live trainer appears, or eligible offers are unaffordable
- **THEN** the lane waits at least five minutes before another trainer travel attempt

#### Scenario: Current trainer has no eligible class spells
- **WHEN** the authoritative list is a class-trainer list with no eligible unknown class spell
- **THEN** the lane clears class training due state
