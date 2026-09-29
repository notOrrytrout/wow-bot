# Add the `.bot clear` command

## Why

Operators need a local command that cancels the active mission and returns the lane to a safe idle state.

## What changes

- Recognize `.bot clear` as a local command for configured accounts.
- Replace the lane mission with a fresh idle mission, then stop bot control.
- Keep unrelated dot commands on the existing forwarding path.

## Capabilities

- Modified: `proxy-local-commands`
