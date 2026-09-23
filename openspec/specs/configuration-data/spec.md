# Configuration and Runtime Data Specification

## Purpose

Define how configuration, bot roster data, navigation assets, and static world knowledge are accepted and used without turning static data into false live-state authority.

## Requirements

### Requirement: Typed configuration validation
The system SHALL reject invalid configuration values and SHALL reject unknown fields where the configuration schema declares unknown fields invalid.

#### Scenario: Unknown denied field is present
- **WHEN** a configuration section contains a field that its typed schema does not permit
- **THEN** configuration validation fails with the offending field identified

### Requirement: Roster-owned bot selection
The system SHALL resolve configured bot and account membership from the roster configuration rather than accepting account credentials as ordinary bot-selection command-line arguments.

#### Scenario: User selects configured bots
- **WHEN** the user selects one or more configured bots for a run
- **THEN** selection resolves against roster entries
- **AND** credentials are not required in the selection arguments

### Requirement: Mandatory runtime-data validation
The system SHALL verify required DBC, map, VMap, mmap, and other configured runtime-data dependencies before features that depend on them are enabled.

#### Scenario: Navigation asset is missing
- **WHEN** a mandatory navigation or collision asset is missing or incompatible
- **THEN** the dependent runtime path fails closed instead of inventing movement geometry

### Requirement: Conventional data-directory resolution
The system SHALL support conventional data children under a configured data root when explicit child paths are not supplied.

#### Scenario: Data root is configured
- **WHEN** a valid data root is configured without explicit child overrides
- **THEN** the system resolves the expected DBC, maps, VMaps, and mmaps child locations


### Requirement: First-run network topology discovery
The supervisor SHALL determine the local deployment topology during first-run setup when the required network settings are not already valid and persisted.

#### Scenario: AzerothCore runs on the same machine
- **WHEN** the user confirms that the bot supervisor and AzerothCore server run on the same machine
- **THEN** the default upstream authentication and world hosts resolve to loopback or another explicitly configured local host
- **AND** the system does not require the user to enter an upstream IP address
- **AND** the bot server's own reachable interface address is discovered automatically when it is needed for remote stock clients

#### Scenario: AzerothCore runs on another machine
- **WHEN** the user confirms that AzerothCore runs on a different machine
- **THEN** setup requires the upstream server host or address
- **AND** the configured upstream authentication and world endpoints use that host instead of loopback

### Requirement: First-run client reachability discovery
The supervisor SHALL determine whether stock WoW clients will connect only from the bot host or from other computers and SHALL derive listener and advertised addresses from that answer.

#### Scenario: Only local clients connect
- **WHEN** the user confirms that stock WoW clients connect only from the bot-server machine
- **THEN** proxy listeners MAY bind to loopback by default
- **AND** the advertised proxy host MAY use loopback

#### Scenario: Other computers connect to the bot server
- **WHEN** the user confirms that stock WoW clients on other computers will connect to the bot server
- **THEN** the proxy SHALL NOT advertise a loopback address to those clients
- **AND** setup SHALL first attempt to discover the bot server's reachable local interface address automatically
- **AND** setup SHALL request a reachable LAN host/address only when automatic discovery cannot produce a usable address
- **AND** proxy listeners SHALL bind to a non-loopback interface or wildcard address suitable for the selected deployment

### Requirement: Upstream service reachability preflight
The supervisor SHALL check the configured AzerothCore authentication and world TCP endpoints before spawning bot workers or declaring the proxy runtime ready.

#### Scenario: Both upstream services are reachable
- **WHEN** startup can establish bounded TCP connections to the configured authentication and world endpoints
- **THEN** startup may continue to worker and proxy activation
- **AND** the successful endpoints are identified in diagnostics

#### Scenario: Authentication server is unavailable
- **WHEN** the configured authentication endpoint refuses, times out, or cannot resolve/connect
- **THEN** startup fails before workers are activated
- **AND** the error identifies the authentication service and configured endpoint
- **AND** the error gives local-server or remote-server troubleshooting guidance appropriate to the configured topology

#### Scenario: World server is unavailable
- **WHEN** the configured world endpoint refuses, times out, or cannot resolve/connect
- **THEN** startup fails before workers are activated
- **AND** the error identifies the world service and configured endpoint
- **AND** startup reports failures for both services when both are unavailable rather than hiding the second failure

### Requirement: Network topology selections are persisted
First-run topology selections SHALL be validated and persisted so later starts do not repeatedly prompt when the saved configuration remains valid.

#### Scenario: First-run topology is accepted
- **WHEN** the user completes the local/remote server and local/remote client questions with valid addresses
- **THEN** the resulting upstream hosts, listener bind addresses, and advertised proxy host are written to configuration
- **AND** later startup reuses those values until they are changed or become invalid

### Requirement: Static knowledge is not live proof
The system SHALL treat generated world knowledge as trusted static guidance, not as proof that a dynamic entity, service, quest state, or object currently exists.

#### Scenario: Static hint names an entity not currently observed
- **WHEN** static world knowledge contains a location or source hint but current state does not prove the entity is present
- **THEN** the hint may guide bounded navigation
- **AND** the system does not treat the entity as currently actionable

### Requirement: Guided first-run configuration
The normal desktop first-run path SHALL collect required setup interactively and SHALL NOT require the user to hand-edit JSON for ordinary initial configuration.

#### Scenario: No configured bot account exists
- **WHEN** the supervisor starts interactively without an enabled bot account
- **THEN** setup requests the WoW account name and password
- **AND** password entry is not echoed to the terminal
- **AND** character name may be supplied without requiring internal identifiers
- **AND** lane and internal account identifiers are assigned by the application
- **AND** the resulting configuration is persisted for later runs

#### Scenario: Setup is already complete
- **WHEN** persisted configuration remains valid
- **THEN** normal startup does not repeat the first-run questions

### Requirement: Bot-owned writable data root
The system SHALL separate read-only AzerothCore runtime-data inputs from wow-bot-owned writable application data.

#### Scenario: AzerothCore data directory is selected
- **WHEN** a user selects the directory containing DBC, maps, VMaps, and mmaps
- **THEN** that directory is recorded as an external runtime-data source
- **AND** wow-bot does not place generated knowledge, logs, cache, state, or its default configuration inside that AzerothCore directory

#### Scenario: Bot creates writable files
- **WHEN** wow-bot creates configuration, generated data, logs, cache, or persistent state
- **THEN** those files are created under `<repo>/wow-bot-data` by default on Windows, macOS, and Linux
- **AND** an explicit bot-data-root override MAY be supported for advanced deployments

### Requirement: Repository-local default writable root
The default wow-bot writable root SHALL be the `wow-bot-data` directory at the repository root on supported desktop operating systems.

#### Scenario: Default root is selected
- **WHEN** `WOW_BOT_HOME` is not set
- **THEN** the application discovers the wow-bot workspace root
- **AND** it uses `<repo>/wow-bot-data` for writable application data
- **AND** the behavior is consistent on Windows, macOS, and Linux
- **AND** `wow-bot-data/` is excluded from source control by the repository `.gitignore`

#### Scenario: Explicit root override is selected
- **WHEN** `WOW_BOT_HOME` is set to a writable location
- **THEN** that location replaces the repository-local default
- **AND** the application still keeps AzerothCore runtime data read-only

### Requirement: Conventional bot-owned subdirectories
The default writable application root SHALL provide distinct locations for configuration, logs, generated artifacts, cache, and runtime state.

#### Scenario: First run initializes writable storage
- **WHEN** the supervisor starts for the first time
- **THEN** it initializes bot-owned `logs`, `generated`, `cache`, and `state` locations
- **AND** the default configuration is stored in the bot-owned root

### Requirement: Native setup dialogs MUST NOT own the long-running supervisor application lifecycle
On desktop platforms, first-run folder selection MAY use a native operating-system dialog, but the dialog implementation MUST terminate when the selection completes and MUST NOT leave the long-running supervisor in a GUI busy/spinning state. On macOS, a terminal/non-windowed supervisor MUST use a short-lived native chooser process or equivalent main-thread-safe mechanism rather than retaining an `NSApplication`-backed dialog lifecycle.

#### Scenario: macOS folder selection returns to normal supervisor execution
- **GIVEN** the supervisor is started from Terminal on macOS
- **WHEN** the AzerothCore data directory must be selected
- **THEN** a native folder chooser is presented
- **AND** the chooser terminates after selection or cancellation
- **AND** the supervisor continues as a normal non-windowed process without a persistent busy/spinning application state
