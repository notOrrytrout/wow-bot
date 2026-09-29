# Project Battleground Status State

Add a typed state foundation and protocol projection for safe battleground queue handling. Keep queue status typed, preserve unknown status values, and clear queue state when the player changes world.

The pinned Tentacli dependency exposes the opcode but no typed message decoder. The adapter decodes the known WotLK body fields from the raw packet body using the old bot's explicit parser offsets. This change does not enable battleground mission actions.
