# Tasks

- [x] Add typed queue status and per-slot battleground state in `wow-state`.
- [x] Keep unknown numeric status values in state and clear only the matching slot for a known empty status.
- [x] Clear battleground queue state on world change and leave-world observations.
- [x] Add reducer tests for multiple slots, unknown states, slot clearing, and world reset.
- [x] Project WotLK `SMSG_BATTLEFIELD_STATUS` packet fields with the old bot's explicit offsets and semantics.
- [x] Add packet fixture tests for active, unknown, and empty-slot status, plus malformed short bodies.
- [ ] Connect queue state to lifecycle policy and mission commands in a separate behavior slice.
