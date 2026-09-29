# Tasks

- [x] Preserve quest reward item IDs from the server offer and reject malformed choice records.
- [x] Add deterministic reward scoring from authoritative item, equipment, class, level, and specialization state.
- [x] Query missing metadata once, wait for a bounded interval, and select only an offered reward after the wait.
- [x] Connect reward selection to the lane action pipeline and verify the selected choice index.
- [x] Add local gather spawn search with current-map and authoritative profession-skill gates.
- [x] Keep static gather locations as search hints and require a live node before gathering.
- [x] Add focused parser, policy, and lane tests for reward selection, metadata fallback, and gather search.
- [x] Run workspace compilation, focused tests, formatting checks, and whitespace checks.
- [ ] Add live server checks for reward metadata timing, reward choice encoding, and gather node visibility at hinted positions.
