# Add typed battleground lifecycle commands

The new bot can now observe authoritative battleground queue status, but its lane cannot express the queue, invite, or exit operations required to use that state. Add typed commands for the supported random battleground workflow and encode them with the WotLK packet layouts used by the legacy client path.

Final action validation binds invite acceptance, queue cancellation, and match exit to the current observed queue slot and battleground type. Only system policy and operator actions may issue these commands, and battleground mission permissions are required.

This change adds the command boundary. The lane lifecycle and match behavior remain a separate implementation step.
