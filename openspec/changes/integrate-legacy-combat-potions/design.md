# Design

Combat policy classifies potions from authoritative item templates and chooses a current backpack item instance. Health potions are eligible at or below 25% health when no legal self-heal is available, and at or below 15% regardless. Mana potions use the old class-role thresholds only when combat work remains active. An authoritative Potion Sickness aura blocks both types. After a potion action is accepted, the lane starts one shared one-hour fallback lockout so health and mana potions cannot follow each other.

Maintenance reuses the existing recovery-supply classifier, exact vendor offer checks, class/level eligibility, one-lot cap, pending inventory confirmation, and money reserve. Health and mana potion reserves target three items. The existing Mage food/drink behavior remains unchanged, and potion restocking remains limited to an already nearby trusted vendor.
