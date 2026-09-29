# Integrate Legacy Recovery Vendor Supplies

Restore safe food, drink, and bandage restocking from the old bot. Use current backpack templates and observed vendor offers. Keep this maintenance opportunistic and local.

## Scope

- Count recognized recovery consumables from authoritative backpack item instances.
- Query current offer templates and buy one affordable server-defined lot from a nearby trusted vendor.
- Preserve 1,000 copper and wait for an authoritative inventory count change, or a bounded timeout, before another recovery-supply purchase.
- Do not route to a vendor or buy items with unknown, unsupported, extended-cost, out-of-stock, class-ineligible, or level-ineligible data.
