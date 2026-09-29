# Add scoped `.bot forget`

## Why

Operators need a local command to remove persistent SQL memory for one configured bot without affecting other bots that share the database.

## What changes

- Recognize `.bot forget` only for a configured account and enabled worker.
- Require the existing protected `debug.enabled` config gate.
- Read the database URL from the configured environment variable and fail clearly when it is unavailable.
- Delete the legacy memory rows for the roster bot ID in one transaction.

## Capabilities

- Modified: `proxy-local-commands`
- Modified: `observability-memory-risk`
- Modified: `private-data-security`
