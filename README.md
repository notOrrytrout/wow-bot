# wow-bot

A Rust-based World of Warcraft bot project for AzerothCore, with quest automation, navigation, and a supervisor that manages multiple account workers.

The project targets the Wrath of the Lich King protocol through Tentacli. It uses server observations to track game state and validate bot actions.

## Project status

This project is under development. The source includes quest discovery, acceptance, objective handling, and turn-in paths, plus ground navigation, controlled movement, buff maintenance, and player-to-bot control handoff. Coverage and live verification remain incomplete.

Full class combat rotations, general quest-item use, group and raid automation, and battleground behavior are not complete. Learned gameplay memory does not currently persist across worker restarts.

See [Project details](docs/PROJECT_DETAILS.md) for the runtime design, implemented paths, and known gaps.
See [Capability traceability](docs/TRACEABILITY.md) for implementation and verification status by baseline specification.

## Requirements

- Rust and Cargo. Follow the [official Rust installation guide](https://rust-lang.org/tools/install/). The repository selects the stable toolchain with Clippy and rustfmt in `rust-toolchain.toml`.
- Running AzerothCore `authserver` and `worldserver` services.
- An AzerothCore runtime-data directory with non-empty `dbc/`, `maps/`, `vmaps/`, and `mmaps/` directories.
- A WoW account and character on the configured server.
- For headless play on a Warden-enabled AzerothCore server, a local WoW 3.3.5a `Wow.exe` image for supported memory checks.

Cargo fetches the Tentacli Git revision specified in `Cargo.toml` and the other build dependencies.

## Quick start

Run these commands from the repository root:

```sh
cargo build --workspace --all-targets
cargo run -p wow-bot-supervisor
```

On first run, setup asks for the AzerothCore data directory, connection details, and the first account. Password entry is hidden. Later runs reuse the saved configuration.

To supply the runtime-data directory directly:

```sh
cargo run -p wow-bot-supervisor -- --data-root /path/to/azerothcore/data
```

The supervisor checks the upstream services, starts one worker per enabled account, and starts the proxy listeners. See [RUNNING.md](RUNNING.md) for setup options, connection behavior, and diagnostics.

## Bot control

In a configured account's chat session through the proxy, use:

| Command | Purpose |
| --- | --- |
| `.bot on` | Enable bot control. |
| `.bot off` | Disable bot control. |
| `.bot status` | Show bot status. |
| `.bot quest` | Request the quest mission. |
| `.log start` | Start a packet/action trace. |
| `.log status` | Show trace status and its path. |
| `.log stop` | Stop the trace. |

For the initial quest smoke test, put the character near an available quest giver, then use `.bot on` and `.bot quest`. See [Testing](docs/TESTING.md) for expected events and live test procedures.

## Runtime files

By default, writable runtime files are stored in `wow-bot-data/`:

```text
wow-bot-data/
  config.json
  logs/
  generated/
  cache/
  state/
```

Set `WOW_BOT_HOME` to use another writable root. The main log is `wow-bot-data/logs/wow-bot.log`. The runtime directory is excluded from Git because it contains local configuration, credentials, logs, and generated data.

The AzerothCore data directory is a separate, read-only input.

## Repository layout

| Path | Purpose |
| --- | --- |
| `apps/wow-supervisor/` | Setup and supervisor executable. |
| `apps/wow-worker/` | Account worker executable. |
| `crates/wow-proxy/` | Auth/world relay and session control. |
| `crates/wow-supervisor/` | Worker management and supervision. |
| `crates/wow-control-proto/` | Control messages between processes. |
| `crates/wow-domain/`, `crates/wow-state/` | Shared types and server-derived game state. |
| `crates/wow-engine/`, `crates/wow-policy/` | Mission execution, action validation, and gameplay policy. |
| `crates/wow-navigation/` | Routes, terrain, and movement control. |
| `crates/wow-tentacli-adapter/` | Tentacli packet and object integration. |
| `crates/wow-infra/` | Configuration, logging, and supporting services. |
| `tools/world-knowledge-gen/` | World-knowledge generation tool. |
| `docs/` | Project details and live test procedures. |
| `openspec/` | Specifications and change proposals. |

## Development checks

```sh
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets
tools/check-binrw-future-compat.sh
```

A successful build does not prove that all gameplay features work. Use the live checks in [Testing](docs/TESTING.md) and review the limits in [Project details](docs/PROJECT_DETAILS.md).
