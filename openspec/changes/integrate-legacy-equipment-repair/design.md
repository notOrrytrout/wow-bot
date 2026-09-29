# Design

The Tentacli adapter reads current and maximum durability only from the 19 authoritative equipment slots. It publishes an observed summary with the lowest percentage and count of broken items. If the equipment list, an item, or its durability fields are missing, it publishes an unknown summary. World changes clear the summary.

Maintenance repairs only observed broken gear or gear below 25 percent durability. It selects a repair vendor from trusted world knowledge, then requires the matching live interactable NPC before it sends a repair action. The action validator checks the observed entity, repair service catalog entry, current durability need, range, mission maintenance permission, and SystemPolicy or Operator origin. The packet uses CMSG_REPAIR_ITEM with the vendor GUID, zero item GUID, and guild-bank use disabled.

The lane waits for an authoritative durability improvement after the request. If no improvement arrives within 15 seconds, it delays another attempt for 60 seconds. It does not start a repair detour while group lifecycle is Active. If the group becomes Active or durability no longer requires repair during a repair route, the lane stops that route.
