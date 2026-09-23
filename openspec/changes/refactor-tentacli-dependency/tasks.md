## 1. Dependency refactor

- [x] 1.1 Remove wow-bot dependency on custom Tentacli `ConnectionEnd`.
- [x] 1.2 Classify intentional shutdown from wow-bot cancellation/control state.
- [x] 1.3 Handle character-creation failure in wow-bot.
- [x] 1.4 Move character-creation request/response logging into wow-bot.
- [x] 1.5 Switch to crates.io `tentacli = 15.2.2`.
- [x] 1.6 Remove the Tentacli `[patch.crates-io]` entry.
- [x] 1.7 Remove `bot/vendor/tentacli`.
- [x] 1.8 Regenerate `Cargo.lock` for the crates.io Tentacli source.

## 2. Validation

- [ ] 2.1 Run full workspace tests.
- [ ] 2.2 Run network lifecycle tests.
- [ ] 2.3 Run `openspec validate --changes`.
