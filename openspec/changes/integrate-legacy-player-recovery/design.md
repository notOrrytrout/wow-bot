# Design: Integrate Legacy Player Recovery

Use the existing maintenance item classifier and authoritative inventory item templates to select food or drink. Keep selection in `wow-policy::maintenance` so item eligibility and resource thresholds have one policy owner. The lane submits `GameplayCommand::UseItemInstance` through the existing action validation and proxy path.

A temporary lane-local recovery lease stores only the selected item and baseline authoritative resource. It waits for a matching observed aura or resource progress, releases when the legacy target is reached, and stops after the bounded timeout or stall interval. Combat, observed transport attachment, controlled mover state, or observed movement preempts the lease and arms a retry delay. The durable mission does not change.

The new state projection has no stand-state field and `GameplayCommand` has no sit/stand action. This slice does not force a sit or infer one. Food or drink activation that requires sitting can fail at the server; the lease then expires or detects stopped aura and waits before another attempt. A future state/action slice can restore the old sit/stand ownership behavior after it adds typed, validated stand-state support.
