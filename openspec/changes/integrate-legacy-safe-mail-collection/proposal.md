# Integrate safe nearby mail collection

## Why

The old bot collected money and attachments from non-COD mail while it was already beside a mailbox. The new lane runtime has mail action state and stale-generation validation, but it does not read mail lists or execute mail actions.

## What changes

- Parse authoritative Wrath mail-list responses, including COD copper, money, and attachment identity.
- Encode typed mailbox-list and one-at-a-time money or attachment actions.
- Collect only from exactly one currently observed trusted mailbox within five yards.
- Reject unknown or nonzero COD, stale mail generations, missing attachments, and attachments when no bag slot is free.
- Wait for authoritative mailbox state after each take and apply bounded timeouts and retry delays.

## Scope

This change does not travel to mailboxes or change trade, auction, or other economy behavior.
