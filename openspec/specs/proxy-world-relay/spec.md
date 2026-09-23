# Proxy World Relay Specification

## Purpose

Define transparent and configured world-session relay behavior, frame forwarding, lane-local upstream recovery, Warden continuity, and authoritative routing between stock clients, bot workers, and AzerothCore.

## Requirements

### Requirement: World traffic is routed by account lane
The proxy SHALL route configured world traffic through the authoritative upstream world session for the matching account and SHALL NOT cross-route gameplay traffic between lanes.

#### Scenario: Two configured accounts are active
- **GIVEN** accounts A and B each have an active lane
- **WHEN** either lane forwards world traffic
- **THEN** its traffic reaches only its own authoritative upstream session
- **AND** ownership changes in A do not redirect B

### Requirement: Unknown-account world sessions remain transparent
The proxy SHALL provide transparent world relay behavior for accounts outside the configured roster.

#### Scenario: Unknown account enters the world
- **WHEN** a non-roster account connects to the transparent world listener
- **THEN** its world authentication and subsequent frames are relayed to the upstream world service
- **AND** configured ownership or bot control is not attached to that session

### Requirement: Configured player world sessions can replace bot sessions without proxy restart
For a configured account, a player world session SHALL be able to become authoritative through AzerothCore's same-account replacement behavior while the proxy service and unrelated lanes remain running.

#### Scenario: Player world session becomes authoritative
- **GIVEN** a configured bot lane already has an upstream WorldSession
- **WHEN** the configured stock client authenticates a fresh upstream WorldSession for the same account
- **THEN** the old bot WorldSession may be replaced by AzerothCore
- **AND** the proxy remains running
- **AND** unrelated listeners and lanes remain bound

### Requirement: Bot requests can use the player-owned authoritative session
When configured player presence exists and bot control is enabled, the proxy SHALL forward permitted bot gameplay requests through the player's already-authoritative upstream WorldSession instead of opening a second concurrent gameplay session.

#### Scenario: Bot assistance is enabled while player stays logged in
- **GIVEN** the configured player session is authoritative
- **WHEN** bot control is enabled for that lane
- **THEN** permitted worker actions are forwarded through the player-owned upstream session
- **AND** the player remains connected
- **AND** no second authoritative gameplay WorldSession is required

### Requirement: Player gameplay remains usable during bot assistance
When bot assistance is active on a player-owned session, normal supported player gameplay packets SHALL continue to reach AzerothCore unless a packet is intentionally proxy-local or movement ownership temporarily assigns locomotion elsewhere.

#### Scenario: Player casts while bot assistance is enabled
- **WHEN** the player sends a normal spell, targeting, cancellation, loot, or other supported gameplay packet
- **THEN** the proxy forwards it according to the shared-session rules
- **AND** bot assistance is not disabled solely because the player performed that non-movement action

### Requirement: Warden exchange remains continuous
The configured player relay SHALL preserve the required Warden exchange during world authentication and live play.

#### Scenario: Warden arrives before auth response
- **WHEN** AzerothCore sends `SMSG_WARDEN_DATA` before `SMSG_AUTH_RESPONSE`
- **THEN** the independent player relay processes the Warden exchange
- **AND** authentication continues instead of dropping the player session

### Requirement: Upstream failure recovery is lane-local
An unexpected upstream world failure SHALL recover only the affected account lane with bounded backoff and SHALL NOT restart shared proxy listeners or unrelated lanes.

#### Scenario: One lane loses its upstream world transport
- **WHEN** the failure is not an expected player takeover
- **THEN** that lane resets its upstream transport and re-authenticates with backoff
- **AND** shared player-auth routing remains available
- **AND** unrelated lane listeners remain available

### Requirement: Expected takeover is not treated as ordinary failure
The proxy SHALL classify a generation-stamped same-account player replacement as player takeover even when current player-presence timing could otherwise make the disconnect look like an upstream failure.

#### Scenario: Presence changes before old bot socket observes EOF
- **WHEN** the old bot world connection ends after a stamped player-takeover transition
- **THEN** it remains classified as takeover
- **AND** it is not reclassified solely from the current presence boolean

### Requirement: Frame forwarding preserves protocol boundaries
The proxy SHALL parse or frame world messages only as needed for routing, control, safety, observation, logging, Warden, or movement behavior and SHALL preserve unrelated payload semantics when forwarding.

#### Scenario: Ordinary unhandled frame is relayed
- **WHEN** a frame does not require proxy-local intervention
- **THEN** the frame is forwarded to its intended peer without being converted into a different gameplay command
