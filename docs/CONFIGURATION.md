# Runtime behavior settings

The supervisor reads `config.toml`. The `runtime_tuning` section controls bounded movement, maintenance, and group behavior.

## Group loot

The bot votes only during a Party or Raid mission. It needs a fresh server roll request, a supported Group Loot or Need Before Greed method, and a choice that the server allows. By default, it passes each roll once.

Use these optional settings to enable non-pass choices:

```toml
[runtime_tuning.group.loot]
need_usable_upgrades = false
greed_non_upgrades = false
disenchant_non_upgrades = false
max_need_quality = 4
```

`need_usable_upgrades` enables Need only when the item is usable, its metadata is known, and authoritative equipment data proves that it improves an equipped slot. `greed_non_upgrades` and `disenchant_non_upgrades` require known metadata and proof that the item is not an upgrade. If the bot does not have enough item or equipment data, it uses Pass when the server allows it. The bot does not vote when the server does not allow Pass and no configured choice is safe.

`max_need_quality` sets the highest item quality for automatic Need votes. The runtime limits this value to 7, the highest WotLK item quality.

Group loot voting is independent of direct loot collection. Existing ownership and lootability checks still control corpse and object loot actions.
