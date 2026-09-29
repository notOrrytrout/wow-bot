# Tasks

- [x] Add typed commands for random queue, status query, invite port, queue cancellation, and match exit.
- [x] Encode commands with the verified WotLK packet layouts from the legacy implementation.
- [x] Require Group mission permission and SystemPolicy or Operator origin.
- [x] Revalidate invitation, queue, and active-match state before state-changing commands.
- [x] Add validation and packet-layout regression tests.
- [x] Use these commands in the lane battleground lifecycle.
- [x] Check for an existing queue before joining and use bounded queue, status, port, and exit retries.
- [x] Accept invitations only with authoritative queue type plus known alive, out-of-combat, on-foot player state.
- [x] Clean up queued or active battleground state when the mission changes.
- [x] Run proactive player combat only on supported battleground maps with battleground mission authority.
- [x] Project full and incremental battleground world-state observations with map, zone, and area identity.
- [ ] Add and verify team-aware objective selection, routing, interactions, and match completion behavior.
- [ ] Verify objective and vehicle behavior against live AzerothCore battleground matches.
