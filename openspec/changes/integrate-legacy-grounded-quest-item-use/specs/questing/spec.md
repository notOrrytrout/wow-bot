## Changed Requirements

### Requirement: Scripted targeted quest-item use waits for authoritative quest credit
Quest item use SHALL match the configured grounded spell against authoritative item-template metadata before dispatch. Missing metadata SHALL be queried and awaited. Conflicting metadata SHALL prevent item use. Objective selection SHALL use the configured target entry and wait for authoritative quest credit on the objective entry.

#### Scenario: Inoculation uses the target entry and waits for the credit entry
- **GIVEN** quest 9303 has an incomplete objective for entry 16534 and a live creature entry 16518
- **AND** item 22962 is authoritative and its template reports use spell 29528
- **WHEN** the objective is selected
- **THEN** the shared targeted item-use action targets entry 16518
- **AND** completion waits for progress on objective entry 16534

#### Scenario: Item template does not confirm the grounded spell
- **WHEN** metadata is missing
- **THEN** the lane requests it and waits without sending item use
- **WHEN** metadata reports a spell other than the quest rule spell
- **THEN** the lane refuses item use
