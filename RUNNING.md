# Running wow-bot

## First run

Build the workspace:

```sh
cargo build --workspace --all-targets
```

Start the supervisor:

```sh
cargo run -p wow-bot-supervisor
```

The normal first-run flow does not require editing JSON.

The supervisor stores its writable files in the repository by default:

```text
<repo>/wow-bot-data/
```

This default is the same on Windows, macOS, and Linux. Set `WOW_BOT_HOME` to use a different bot-owned root.

The bot-owned directory contains `config.json`, `logs/`, `generated/`, `cache/`, and `state/`. AzerothCore's data directory is a separate, read-only input. wow-bot must not write generated files into the AzerothCore directory.

On first run, setup:

1. Opens a native folder picker if the AzerothCore runtime-data location is not valid. Select the directory containing non-empty `dbc/`, `maps/`, `vmaps/`, and `mmaps/` directories.
2. Asks whether AzerothCore runs on the same computer.
3. Asks whether WoW clients on other computers will connect to the bot server.
4. Requests a reachable hostname/IP when remote connectivity requires one.
5. Requests the first WoW account name, hides password entry, and optionally requests the character name.
6. Assigns internal lane/account IDs automatically and saves the setup.

Later runs reuse the saved setup.

## Headless or advanced setup

Provide the AzerothCore data directory directly:

```sh
cargo run -p wow-bot-supervisor -- --data-root /path/to/azerothcore/data
```

Use a custom config location only when needed:

```sh
cargo run -p wow-bot-supervisor -- --config /path/to/config.json
```

The bot's writable root remains `<repo>/wow-bot-data` even when `--config` points elsewhere, unless `WOW_BOT_HOME` explicitly overrides it.

## Normal startup

```sh
RUST_LOG=info cargo run -p wow-bot-supervisor
```

The supervisor binds the worker-control socket, starts one worker process per enabled account, starts player auth/world listeners, starts the transparent-world listener, and clears worker startup gates after routing is ready.

Set `RUST_LOG=debug` or `RUST_LOG=trace` for more diagnostics.

## World-knowledge generator

With no output argument, generated knowledge is written under the bot-owned `generated/` directory:

```sh
cargo run -p world-knowledge-gen
```

An explicit output path is still supported for development/testing.

## Startup network checks

Before workers are spawned, the supervisor verifies that both configured AzerothCore services accept TCP connections. If `authserver` or `worldserver` is down, startup stops with the failed service name, endpoint, and topology-specific guidance.

When AzerothCore runs on the same machine, upstream auth/world hosts are configured as loopback automatically. If WoW clients on other computers will connect, setup automatically detects this bot machine's reachable IPv4 address and advertises it; manual address entry is only used as a fallback.

## Logs and local chat commands

The supervisor writes the normal runtime log to the bot-owned log directory and also prints the same events to the console.

The default locations are:

```text
<repo>/wow-bot-data/logs/wow-bot.log
<repo>/wow-bot-data/logs/action-<ACCOUNT>-<timestamp>.jsonl
```

If `WOW_BOT_HOME` is set, the log directory is `$WOW_BOT_HOME/logs/` instead.

Configured-account chat commands such as `.bot on`, `.bot off`, `.bot status`, `.log start`, `.log stop`, `.log status`, and `.log mark` are consumed by the proxy and are not forwarded to AzerothCore. Each recognized command is printed to the supervisor console and written to `wow-bot.log`.

`.log start` creates a per-account JSONL packet/action trace in the same log directory. The console prints the exact path. `.log status` prints whether action logging is active and its path. `.log stop` closes the trace and prints its path.

## Quest command visibility

`.bot quest` is not considered successful only because the proxy accepted the command. A runnable lane reports mission installation and then either emits quest protocol actions or an explicit `mission scheduler waiting` reason. With an available quest giver in interaction range, the first supported autonomous path queries quest-giver status, opens the giver, parses the authoritative quest list, and sends a validated quest-accept request.

### Worker binary discovery

When running from a source checkout, `cargo run -p wow-bot-supervisor` resolves the worker from the current workspace. If `wow-bot-worker` has not been built yet, the supervisor builds that package automatically and continues. Packaged builds expect `wow-bot-worker` beside the supervisor executable. `--worker-bin <path>` remains available as an override.

## Generated runtime data

After the AzerothCore data root is validated, the supervisor reads the external `dbc/`, `maps/`, `vmaps/`, and `mmaps/` directories and writes a derived asset manifest to:

```text
wow-bot-data/generated/runtime-assets.json
```

The AzerothCore data tree is read-only. The generated manifest contains source paths, file counts, modification metadata, and discovered map IDs. It is static guidance/provenance only and never proves that a live NPC, quest, or game object exists.

Configured world sessions now use the real `CMSG_PLAYER_LOGIN` character GUID and AzerothCore `SMSG_LOGIN_VERIFY_WORLD` position. Tentacli's WotLK object processor also projects visible object updates into the owning worker's authoritative reducer.


## macOS first-run folder picker

On macOS, the supervisor uses a short-lived native AppleScript folder chooser for first-run AzerothCore data selection. This avoids attaching the long-running terminal supervisor to an `NSApplication` dialog lifecycle. After selection, the chooser exits and the supervisor continues normally.


### Runtime data selection

Each extracted source checkout has its own repo-local `wow-bot-data/config.json`. If that checkout does not yet contain a saved AzerothCore runtime-data path, startup prints a terminal prompt. Paste/type the directory containing `dbc/`, `maps/`, `vmaps/`, and `mmaps/`, or press Enter to open the native folder browser. The selected source directory is read-only; generated bot data stays under `wow-bot-data/`.

### Quest execution diagnostics

Quest mode now distinguishes live interaction authority from static search guidance. AzerothCore-derived static coordinates are used only to approach/search an area. Actual attack, loot, game-object use, quest-giver interaction, and turn-in require a live authoritative entity GUID from the configured WorldSession.

For item quests, the worker derives current counts from Tentacli item/container objects owned by the active character. For completed quests, the worker follows AzerothCore's request-items and offer-reward dialogs before considering turn-in complete.


### Quest item, gather, and controlled-unit objectives

Quest collection objectives use live inventory counts from item/container objects owned by the configured character. AzerothCore-derived loot and spawn metadata are search guidance only; the bot still requires a live creature or game-object GUID before it attacks, loots, gathers, or interacts.

Special controlled-unit quests use a separate active-mover state. The runtime observes the controlled GUID, its position and movement flags, and the controlled action bar from server packets. For Eye-of-Acherus-style objectives, the resolver can select an observed applicable controlled ability for a live objective marker instead of treating the marker as a normal combat target. Bot-controlled movement uses the controlled mover while that state is authoritative.

Generic quests that require using a quest item on a target are not yet treated as the same thing as item collection. They remain a separate incomplete action semantic and must not be reported as supported only because inventory counting works.

## Shared movement and scripted quest behavior

Ground quest movement now requires the worker's `--maps-dir` path and samples the selected read-only AzerothCore `maps/` terrain for each movement step. If terrain cannot be sampled or a vertical discontinuity is unsafe, movement fails closed and logs a waiting reason instead of inventing Z/flying.

Controlled movers with authoritative flying/can-fly/disable-gravity flags use the same movement controller in flight mode. This is the path used by the Eye of Acherus flow for quest 12641.

Scripted quest mechanics currently include:
- quest 5441 (Lazy Peons): authoritative backpack item instance -> targeted item-use spell -> wait for objective credit;
- quest 12641 (Death Comes From On High): spell-targeted quest control-object activation -> wait for authoritative controlled mover -> controlled flight -> observed controlled ability -> wait for objective credit.

## Shared spatial preconditions

Targeted actions now use one canonical spatial pipeline:

```text
range -> authoritative LOS/range recovery -> facing -> action
```

When the target is out of range, the original action becomes the resume intent of owned movement work. When AzerothCore reports `SPELL_FAILED_LINE_OF_SIGHT`, `SPELL_FAILED_OUT_OF_RANGE`, or `SPELL_FAILED_TOO_CLOSE` for a recent bot-targeted spell/item-use/control cast, the same shared spatial layer performs deterministic repositioning through the normal movement controller. If only facing is missing, the worker emits `MSG_MOVE_SET_FACING` through the same movement-generation/transport fencing path and revalidates the original action afterward.

This path is shared by questing (including Lazy Peons and Eye of Acherus), ordinary casts/interactions, gathering, combat, and future group/PvP callers. Mission-specific code should not implement separate range/facing/LOS mechanics.

### Bot memory status

The current runtime has rich authoritative and mission state in RAM while a worker is alive, plus persistent logs/configuration. A `MemoryStore` abstraction and secure `RemoteMemory` interface exist, but they are not yet connected to lane behavior. Therefore learned gameplay experience does **not** currently survive a process restart in a usable bot-memory form. Treat durable learned memory as a separate incomplete foundation feature; do not confuse action logs or static world knowledge with bot memory.


### Multi-level ground navigation

Routed ground movement treats the Detour/MMAP corridor as the vertical authority. Route points retain optional `RouteSurface` identities (map/tile/polygon), and small execution steps are re-projected onto adjacent authorized route polygons when possible. Raw `maps/` terrain remains authoritative for unrouted ground movement, but it does not overwrite a valid routed cave/bridge/interior floor. This prevents stacked geometry from being snapped onto an unrelated outdoor terrain layer.
