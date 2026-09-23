## 1. Local Tentacli dependency

- [x] 1.1 Restore `bot/vendor/tentacli` as the local dependency source.
- [x] 1.2 Set the vendored source to version `15.2.3`.
- [x] 1.3 Point wow-bot at `bot/vendor/tentacli` with a path dependency.
- [x] 1.4 Exclude the vendored Tentacli crate from root workspace membership.
- [x] 1.5 Update `Cargo.lock` for the local path package.
- [x] 1.6 Preserve config-backed deterministic character creation request compatibility.

## 2. Object API

- [x] 2.1 Include the public WotLK object module and `ObjectMap` helpers.
- [x] 2.2 Include typed object/unit/player/game-object accessors.
- [x] 2.3 Remove out-of-range GUIDs from `ObjectMap`.
- [x] 2.4 Include external-consumer object API tests in the vendored crate.

## 3. Validation

- [ ] 3.1 Run `cargo fmt --all --check`.
- [ ] 3.2 Run `RUSTFLAGS="-D warnings" cargo check --workspace`.
- [ ] 3.3 Run `cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] 3.4 Run `RUSTFLAGS="-D warnings" cargo test --workspace`.
- [ ] 3.5 Run `openspec validate --changes`.
