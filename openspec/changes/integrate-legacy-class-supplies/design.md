# Design

Mage supply policy runs after higher-priority pet and gear work and before ordinary buff work. It uses the current snapshot, authoritative known-spell set, shared spell readiness checks, and `MaintainBuff` action path. The lane applies a ten-second retry delay after a Conjure attempt, then waits for authoritative backpack state before deciding whether to cast again.

Item-template parsing retains the primary item name and first usable item spell. These fields support food, drink, and stone classification without changing packet ownership. Policy returns typed item-use work for Soulstone; the lane sends it through the existing action validator and item packet path.
