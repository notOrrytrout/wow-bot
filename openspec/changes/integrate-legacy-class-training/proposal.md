# Integrate legacy class trainer learning

## Why

The legacy bot learned eligible class spells from nearby trainers and then used authoritative spellbook updates in combat policy. The lane-oriented bot has authoritative spellbook and money state, but it did not observe trainer offers or perform class training.

## What changes

- Preserve NPC trainer flags from observed unit updates.
- Parse the current WotLK trainer list into authoritative state.
- Add typed list and buy actions to the shared validation and dispatch path.
- Let maintenance request a nearby class trainer list and buy one eligible, affordable spell at a time.
- Keep profession training and long-distance trainer travel as separate policy work.
