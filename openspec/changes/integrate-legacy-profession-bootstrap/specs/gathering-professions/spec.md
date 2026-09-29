## ADDED Requirements

### Requirement: Autonomous profession bootstrap uses live trainer authority
The system SHALL preserve learned primary professions and may acquire missing selected skills through bounded profession training. Static catalog mappings SHALL classify profession trainer offers only; a purchase SHALL require a live nearby catalog-backed profession trainer and its current eligible, affordable offer.

#### Scenario: A matching profession offer is available nearby
- **WHEN** automatic profession training is enabled, the bootstrap level gate is due, selected profession state is authoritative, a trusted profession trainer is live and nearby, and its current list contains an eligible affordable spell mapped to a missing selected skill
- **THEN** the system may buy that offer while preserving the lane purchase reserve and maintenance safety gates

#### Scenario: An offer is not mapped to a selected missing profession
- **WHEN** a profession trainer lists a spell with no trusted trainer spell-to-skill mapping or the mapped skill is not selected and missing
- **THEN** the system SHALL NOT buy that spell

#### Scenario: Profession bootstrap is not due or safe
- **WHEN** the character is below level ten and has no eligible early Cooking, Mining, or Skinning goal, or the lane is in combat, moving, grouped, lacks permissions, lacks reserve funds, or has a nearby quest offer
- **THEN** the system SHALL NOT route to or train at a profession trainer

#### Scenario: First Aid cannot block higher-priority goals
- **WHEN** First Aid is the only unavailable selected skill
- **THEN** the system leaves ordinary mission progression eligible and does not keep retrying it ahead of Cooking, Fishing, or selected primary professions
