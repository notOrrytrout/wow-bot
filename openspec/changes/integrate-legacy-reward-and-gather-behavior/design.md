# Design

Quest reward offers carry their item IDs from the proxy observation parser into authoritative quest state. The lane asks for missing reward and equipped-item templates through the existing `QueryItem` action. It requests each missing template once, waits for a bounded eight seconds, and then selects the best usable equipment upgrade. If no offered item is a proven upgrade, it uses vendor value and item level as a deterministic fallback. The chosen value remains the zero-based index from the server offer and goes through normal action validation.

Gather search matches the configured resource name against the reviewed AzerothCore gathering catalog. It checks the authoritative mining, herbalism, or fishing rank, restricts search positions to the active mover's current map, and uses the existing bounded search and movement lifecycle. A static position only guides movement. The bot waits for a matching live game object before it sends a gather action.

Both behaviors reuse current domain types, worker state, item scoring, search retry state, and navigation. They do not add a second movement or action execution path.
