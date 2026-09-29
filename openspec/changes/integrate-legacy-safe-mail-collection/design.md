# Design: safe nearby mail collection

The lane uses the existing `MailTake` command, bound to the observed mailbox generation, mail ID, and one explicit target. Money and attachments use their separate Wrath client opcodes. Attachment actions carry the low GUID supplied by the mail-list response.

The proxy parses `SMSG_MAIL_LIST_RESULT` into authoritative state. The parser preserves COD copper as `Option<u64>`; the lane collects only `Some(0)`. It preserves each attachment low GUID with its item ID and stack count. A malformed list or unsupported message type produces no observation.

The lane starts work only when one trusted catalogued mailbox object is observed within five yards. It sends a list request when the current mailbox contents are not authoritative for that object. It then selects one operation: attached money first, then one attachment if at least one bag slot is free. It waits for a mailbox generation change before it can select another operation. A 15-second timeout ends the wait; a 30-second retry delay bounds another request or take attempt. The active mission intent does not change.

The list response does not echo the mailbox GUID. The lane therefore permits only one nearby trusted mailbox during collection and revalidates that object before each state-changing action.
