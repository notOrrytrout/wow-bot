### Requirement: Low-resource food and drink recovery is grounded and preemptible
The lane SHALL select food only when authoritative health is at or below 55%, or drink only when authoritative mana is at or below 35%. If both resources are low, food SHALL be selected first. The lane SHALL use only a known food or drink item from an authoritative backpack instance with authoritative use metadata, class eligibility, and level eligibility. It SHALL require authoritative alive, out-of-combat state and SHALL NOT start while moving, while attached to an observed transport, or while a controlled mover is active.

After dispatch through the shared typed item action, the lane SHALL retain temporary execution ownership while it waits for an authoritative item aura or health/mana progress. It SHALL release ownership at 90% health or 85% mana. The lease SHALL stop after a 32-second deadline or after six seconds without resource progress following a two-second start grace, and it SHALL delay another attempt for ten seconds after a failed attempt. Combat, movement, observed transport attachment, or controlled-mover takeover SHALL preempt the lease. Recovery SHALL NOT replace the durable mission.

#### Scenario: Health and mana are both low
- **WHEN** authoritative health is at or below 55% and authoritative mana is at or below 35%
- **AND** a usable food instance is available
- **THEN** the lane selects the first authoritative backpack food instance
- **AND** it dispatches the item through `UseItemInstance` and preserves the current mission

#### Scenario: Food or drink recovery is active
- **WHEN** the lane has dispatched a low-resource food or drink item
- **THEN** it waits for authoritative aura or resource progress
- **AND** it holds the lane until the resource reaches its target, recovery stalls or times out, or a higher-priority combat, movement, or control event preempts it

#### Scenario: Item activation needs a sit state the lane cannot control
- **WHEN** food or drink use is rejected or produces no authoritative recovery progress
- **THEN** the lane releases the recovery lease after the bounded stall or timeout
- **AND** it waits through retry backoff before another attempt
- **AND** it does not invent or send a stand-state command
