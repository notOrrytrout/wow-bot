# Project details

This page describes the current runtime and its main limits. The [README](../README.md) gives the quick start, [Running](../RUNNING.md) explains setup, and [Testing](TESTING.md) lists checks.

## Runtime flow

1. `wow-bot-supervisor` loads configuration, checks the AzerothCore services, starts a worker for each enabled account, and starts the proxy listeners.
2. `wow-proxy` owns the account's network session. It handles client and server frames, local chat commands, player handoff, and the headless connection.
3. `wow-tentacli-adapter` turns supported WotLK packets and objects into observations. `wow-state` applies those observations to the worker's game state.
4. `wow-engine` selects mission work and validates each action against current state, permissions, ownership, and movement generations. The proxy checks those fences again before it sends gameplay packets.
5. `wow-navigation` uses AzerothCore map terrain and MMAP routes for ground movement. A controlled mover can use flight only when server state permits it.

Static world data can guide a search. A live server observation must identify a creature, game object, or item before the bot acts on it or counts progress.

## Current gameplay paths

- Quest work includes quest-giver discovery, quest accept, objective tracking, live target selection, movement, loot, and reward steps. Some quest types have specific handling, including Lazy Peons and the Eye of Acherus control sequence.
- The worker can pause bot control when the player takes control and can resume after the configured handoff conditions.
- Headless world entry installs a Quest mission after the server confirms entry into the world. On the supported AzerothCore WotLK Warden module, the proxy checks the downloaded module and answers known check types. Set `proxy.warden_client_image` to a local WoW 3.3.5a `Wow.exe` to answer memory checks. Unknown modules and checks end the session.
- The bot keeps authoritative game state in memory during a worker run. Logs and configuration persist, but learned gameplay memory is not connected to mission behavior after a restart.

## Limits and verification

The project remains under development. Full class combat rotations, general quest-item use, group and raid automation, and battleground behavior are incomplete. Packet coverage and live tests do not cover every server event or quest. The Warden client covers the known AzerothCore WotLK module and check types; it does not claim support for other server variants.

Run the workspace checks in [Testing](TESTING.md), then use its live scenarios for the gameplay path you changed. A passing Cargo test shows that the checked code builds and its automated tests pass. It does not prove a full quest or account session works on a live server.

## Dependency note

The workspace pins Tentacli 15.3.2. Tentacli still requires `binrw` 0.14, so `vendor/binrw-compat` provides that package interface by re-exporting `binrw` 0.15.2. `tools/check-binrw-future-compat.sh` makes the Rust lint that prompted this bridge an error during a workspace check.
