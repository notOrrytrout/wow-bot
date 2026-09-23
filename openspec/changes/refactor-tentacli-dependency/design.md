# Design

## Dependency boundary

Use the crates.io `tentacli = 15.2.2` package. Keep the existing local `binrw` compatibility patch. Remove the local Tentacli patch and vendored Tentacli source.

## Network termination classification

Classify local shutdown from wow-bot's cancellation/control state before inspecting transport errors. This avoids depending on a custom Tentacli `ConnectionEnd` type.

## Character creation

Observe character-creation packets in wow-bot's ordered packet dispatch:

- log outbound `CMSG_CHAR_CREATE`,
- log inbound `SMSG_CHAR_CREATE`,
- accept only the exact success status,
- fail the wow-bot dispatch task for malformed or rejected creation responses.

This keeps character-creation failure semantics in wow-bot instead of in a Tentacli fork.
