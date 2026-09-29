# Integrate Legacy Player Recovery

Restore safe, opportunistic food and drink recovery from the old bot. Select only an authoritative usable backpack instance when observed health or mana is low. Use the new lane's typed item action and its existing validation path.

## Scope

- Use food at or below 55% authoritative health and drink at or below 35% authoritative mana; prefer food when both are low.
- Start only for an alive, stationary, unattached, uncontrolled player with authoritative out-of-combat state.
- Hold the lane while the server shows recovery progress, then release at 90% health or 85% mana. Bound the lease and retry after stalls.
- Preempt on combat, transport attachment, control takeover, or movement, and keep the durable mission unchanged.
- Do not add a stand-state command. The new architecture does not observe or control stand state; the server remains authoritative about whether food or drink can start.
