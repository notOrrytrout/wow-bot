## Why

Solo combat admission was too dependent on raw hostile counts and route exposure. A low-level bot could treat a higher-level pack too lightly, while a higher-level bot could flee from trivial low-level enemies only because several were nearby. Gear condition was visible in diagnostics but not part of a reusable encounter-power comparison.

## What Changes

- Add a reusable encounter-power budget that compares player effective power against observed hostile pack power.
- Include player level, hostile level range, hostile count, elite/rank contribution, equipment-condition contribution, health, resource pressure, role/profile survivability, and uncertainty.
- Use the result in solo combat survival escalation for both melee and ranged/caster profiles.
- Preserve stricter ranged/caster treatment and the existing solo ranged kite-back behavior.
- Emit diagnostics that explain the encounter-power decision.
- Persist recent mob-death risk in MySQL/local memory so a creature entry and location that recently killed the bot raises future encounter risk.

## Impact

- High-level solo bots do not flee from low-level packs solely because the count is high.
- Low-level melee bots flee or avoid clearly overpowering higher-level packs.
- Ranged/caster solo profiles remain more conservative than melee for comparable pack power.
- Missing gear scoring remains neutral but increases uncertainty.
