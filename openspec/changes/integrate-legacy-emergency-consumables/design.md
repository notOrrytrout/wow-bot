# Design

The combat lane selects healthstones before potions when health is at or below 30 percent. It selects a Whipper Root Tuber or Night Dragon's Breath only after the potion selector declines, using the same 25-percent/no-self-heal or 15-percent emergency gate as health potions. Healthstones have a five-second retry window. The two non-potion items share a separate 30-second retry window. Neither non-potion path reads Potion Sickness or the shared potion timer.

Selection uses the current item instance and item-template data, including backpack slot and use spell. The lane sends the existing `UseItemInstance` command on the player. Combat confirmation remains a lane concern; survival recovery actions use the existing recovery authority.
