# Design

## Dependency source

Use `bot/vendor/tentacli` as a local path dependency while the WotLK object accessor API is not released on crates.io. The vendored crate version is `15.2.3`.

The root workspace excludes `bot/vendor/tentacli` from workspace membership so Cargo treats it as an external path dependency. The root workspace keeps only the existing `binrw` compatibility patch. `tentacli` is selected directly through `bot/Cargo.toml` as a path dependency.

## Object API

The vendored source includes the upstream public WotLK object module shape, `ObjectMap`, `objects()`, and `objects_mut()`. It adds typed read-only getters on `Object` for common object, unit, player, and game-object fields. Object map updates remove out-of-range GUIDs before applying creates and value updates.

## Boundary

This does not make Tentacli responsible for wow-bot semantic gameplay state. wow-bot continues to own semantic projection, lifecycle classification, and character-creation response validation/logging. The local Tentacli dependency supplies canonical decoded WotLK object state for future adapter work.

## Character creation request compatibility

The local Tentacli source keeps the existing config-backed deterministic character creation request fields that wow-bot writes into `connection.toml`. Tentacli only builds the outbound `CMSG_CHAR_CREATE` request. wow-bot remains responsible for character-creation response validation and diagnostics.
