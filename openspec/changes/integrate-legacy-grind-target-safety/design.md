# Design

The lane delegates Grind target selection to pure `wow-policy` logic. The selector reads the current `Snapshot`, uses the active mover position when present, requires a finite position on the same map, and caps voluntary selection at 100 yards. It orders eligible candidates by distance and then `EntityId` so repeated evaluations are deterministic.

The legacy level rule rejects a target only when both player and target levels are known and the target is more than two levels above the player. Missing player health, mana, maximum values, or target level do not create invented evidence. Known health below 45 percent or mana below 20 percent blocks a pull. A cluster of at least three other hostiles within nine yards blocks a pull; a smaller cluster requires known reserves of at least 75 percent health and 45 percent mana where those values are available.

The current entity model has neither creature rank nor observation timestamps. The selector does not infer elite/boss rank from names or unrelated flags. It treats retained entity entries and their current finite positions as the available observation, and current action validation remains responsible for checking the selected target before send. This means the lane cannot independently screen elite/boss creatures or quantify observation age until state gains those fields.

An engaged player or any observed hostile attacking the player or an online group member blocks voluntary Grind selection. This leaves that encounter to existing combat and self-defense policy. Candidate filtering remains limited to `Unit` entities, so ordinary Grind does not select player entities.
