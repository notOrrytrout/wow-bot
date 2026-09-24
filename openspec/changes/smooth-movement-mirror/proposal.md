# Smooth bot movement and client mirroring

## Why

Movement currently advances and mirrors in large, infrequent position updates. This makes the attached client look delayed or jerky. The old bot also protects client-facing movement with ordered updates and recovery when updates are missed.

## What changes

- Keep the existing navigation and route step planner.
- Increase active movement update frequency and reduce each route step to preserve ordinary ground-run speed.
- Queue the latest client mirror update separately from upstream packet transmission.
- Sequence mirror updates, fence them when ownership or the mission changes, and send the latest position after skipped updates.
- Reject invalid movement poses before packet encoding and report mirror write failures with account and sequence context.

## Impact

The changes affect worker movement timing, navigation step size, and the configured player world bridge. They do not change route selection or the transparent relay.
