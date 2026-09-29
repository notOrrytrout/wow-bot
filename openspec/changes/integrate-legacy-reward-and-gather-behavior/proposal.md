# Integrate legacy reward and gathering behavior

## Why

The old bot compared quest rewards with current equipment and used reviewed resource spawn data to search when no gather node was visible. The lane-oriented bot had the needed item scoring, authoritative profession state, navigation, and shared action path, but it did not connect these behaviors to mission execution.

## What changes

- Preserve item IDs from authoritative quest reward offers.
- Select the strongest usable equipment upgrade and use a bounded metadata wait with a deterministic fallback.
- Use trusted local gather spawns as movement hints when no matching live node is visible.
- Keep live observations, profession rank, and the shared action validator as requirements for gathering.

## Scope

This change covers reward selection and local gathering search. It does not implement the remaining player service, training, group, battleground, model-planning, or commerce work identified in the legacy behavior comparison.
