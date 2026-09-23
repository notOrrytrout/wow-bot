# Proxy Authentication Specification

## Purpose

Define downstream stock-client authentication, configured-account authentication termination, upstream AzerothCore authentication, realm selection, admission limits, and transparent authentication for accounts outside the enabled bot roster.

## Requirements

### Requirement: Configured accounts use proxy-terminated authentication
For an enabled configured account, the proxy SHALL terminate the stock client's downstream authentication and establish the upstream authenticated context required for the account lane without exposing bot credentials to the stock client.

#### Scenario: Configured stock client authenticates
- **GIVEN** an account belongs to an enabled configured lane
- **WHEN** a stock client authenticates through the player-facing auth listener
- **THEN** the proxy authenticates the downstream client according to the configured-account flow
- **AND** the resulting player world route is associated only with that account lane

### Requirement: Upstream authentication uses the configured WotLK identity
The proxy SHALL authenticate to AzerothCore using the configured account credentials and the supported WotLK 3.3.5a authentication protocol.

#### Scenario: Upstream authentication succeeds
- **WHEN** AzerothCore accepts the configured account challenge and proof
- **THEN** the proxy obtains an authenticated session key
- **AND** it requests the upstream realm list before establishing a world session

#### Scenario: Upstream authentication fails
- **WHEN** challenge, proof, realm discovery, or transport authentication fails
- **THEN** the attempt fails explicitly
- **AND** the proxy does not invent a session key or authenticated state

### Requirement: Configured realm selection is explicit
The proxy SHALL select the configured realm by case-insensitive realm name and SHALL fail when that realm is not returned by the upstream auth server.

#### Scenario: Configured realm exists
- **WHEN** the upstream realm list contains the configured realm name
- **THEN** that realm supplies the authoritative realm identifier
- **AND** its world port may be used with the configured upstream world host

#### Scenario: Configured realm is absent
- **WHEN** the upstream realm list does not contain the configured realm name
- **THEN** authentication fails with an actionable error


### Requirement: Proxy endpoints reflect deployment topology
The proxy SHALL separate upstream AzerothCore addresses, local listener bind addresses, and client-advertised proxy addresses so each can be correct for local or multi-host deployments.

#### Scenario: AzerothCore and bot server share a host
- **GIVEN** the bot server and AzerothCore run on the same machine
- **WHEN** configured-account authentication connects upstream
- **THEN** the upstream host may use loopback while downstream listeners independently use the configured client-facing bind addresses

#### Scenario: Remote clients connect through the bot server
- **GIVEN** one or more stock clients run on machines other than the bot server
- **WHEN** the proxy returns or rewrites a client-facing realm/world address
- **THEN** it uses the configured reachable advertised proxy host
- **AND** it does not expose `127.0.0.1`, `::1`, or another loopback-only address to remote clients

#### Scenario: AzerothCore is remote from the bot server
- **GIVEN** AzerothCore runs on another host
- **WHEN** the proxy performs upstream authentication or world connection
- **THEN** it uses the configured upstream AzerothCore host
- **AND** it does not substitute the bot server's advertised client-facing address for the upstream server address

### Requirement: Bot-host address discovery for remote clients
When remote stock clients are enabled, the proxy setup SHALL prefer a discovered non-loopback address of the bot-server machine rather than requiring users to manually transcribe that machine's address.

#### Scenario: Reachable bot-host address is discovered
- **GIVEN** remote stock clients are enabled
- **WHEN** the bot server can identify a non-loopback local interface address
- **THEN** setup uses that address as the default client-advertised proxy host
- **AND** the user is informed which address was selected

#### Scenario: Bot-host address discovery fails
- **GIVEN** remote stock clients are enabled
- **WHEN** no usable non-loopback address can be determined
- **THEN** setup requests a reachable hostname or address from the user
- **AND** it never silently substitutes loopback for remote clients

### Requirement: Configured upstream host remains authoritative
The proxy SHALL keep the configured AzerothCore host authoritative even when the realm list advertises a local or otherwise unsuitable host address.

#### Scenario: Realm advertises localhost
- **GIVEN** the configured AzerothCore host is remote or otherwise explicitly selected
- **WHEN** the realm table advertises `127.0.0.1` or another different host
- **THEN** the proxy retains the configured host
- **AND** may use the realm-advertised world port

### Requirement: Unknown accounts remain transparent
Accounts outside the enabled configured roster SHALL retain transparent end-to-end authentication with AzerothCore and SHALL NOT gain configured bot-control capabilities.

#### Scenario: Unknown account authenticates
- **WHEN** a stock client logs in with an account that is not an enabled configured lane
- **THEN** its SRP exchange remains end to end with AzerothCore
- **AND** the account is not assigned configured bot ownership state
- **AND** proxy-local bot-control commands are not granted to that account

### Requirement: Pre-authentication connections are bounded
The proxy SHALL bound unauthenticated downstream connections globally and per source IP.

#### Scenario: Global pre-authentication limit is reached
- **WHEN** accepting another unauthenticated connection would exceed the configured global limit
- **THEN** the new connection is rejected before expensive authentication work begins

#### Scenario: Per-IP pre-authentication limit is reached
- **WHEN** one source IP already holds the maximum allowed unauthenticated connections
- **THEN** another unauthenticated connection from that IP is rejected
- **AND** permits are released when existing attempts end

### Requirement: Authentication has a bounded handshake time
The proxy SHALL apply the configured handshake timeout to authentication work that must not wait indefinitely.

#### Scenario: Authentication peer stalls
- **WHEN** an authentication peer does not complete the required handshake within the configured deadline
- **THEN** the connection is terminated as a timeout
- **AND** its admission capacity is released
