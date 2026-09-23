# Use local Tentacli object accessor build

## Summary

Point wow-bot at a repository-local Tentacli source while the WotLK object accessor API is still unreleased on crates.io.

## Motivation

wow-bot needs the typed WotLK object accessor API before it is available as a crates.io release. The local dependency uses the maintainer's `15.2.3` test patch source and keeps the dependency inside the wow-bot tree for reproducible local validation.

## Scope

- Restore `bot/vendor/tentacli` as a local dependency source.
- Use the maintainer's `tentacli` `15.2.3` test patch with public WotLK object accessors and lifecycle metadata.
- Keep wow-bot lifecycle classification and character-creation response handling in wow-bot.
- Keep the existing local `binrw` compatibility patch.
