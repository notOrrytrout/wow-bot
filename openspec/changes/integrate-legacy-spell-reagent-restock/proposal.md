# Integrate legacy spell reagent restocking

Restore local, opportunistic restocking for reagents used by known automated class and combat spells. Derive needs from the current spell catalogue and authoritative known spell set.

## Behavior

- Keep five casts of each vendor reagent while solo and ten when grouped, using the largest reagent cost across known ranks.
- Buy only one affordable server-defined lot from an already nearby vendor, after current vendor offers are observed.
- Preserve the 1,000-copper maintenance reserve and wait for the existing bounded vendor-purchase confirmation path.
- Do not buy Soul Shards or travel to obtain reagents.
- Do not turn profession crafting materials into class-maintenance purchase targets.
