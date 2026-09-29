# Design

- Keep bootstrap policy in `wow-policy::gathering::professions` and current lane service dispatch in `wow-engine`.
- Preserve generated DBC `SkillLineAbility`/`Spell` resolution per trainer spell. Static data classifies an offer but never authorizes a trainer interaction.
- Require a live catalog-backed profession NPC within interaction range, trainer type 2, matching live offer, unknown spell, level and prerequisite eligibility, sufficient funds after the lane reserve, and safe solo maintenance conditions.
- Reuse the existing `TrainerList` and `TrainerBuy` actions. Keep class trainer type-0 behavior separate.
- Use a short same-map catalog route only when its point is within the 30-yard local detour bound. Do not add cross-map travel.
- Keep `auto_professions_enabled` in the existing `MaintenanceTuning` boundary, default-on. First Aid is ranked last and does not block other goals.
