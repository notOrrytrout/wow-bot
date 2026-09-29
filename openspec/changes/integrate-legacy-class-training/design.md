# Design

The lane selects only a currently observed trainer NPC within five yards. Current NPC service flags identify trainer units. The server's trainer-list response is the authority for offer usability, cost, level, skill requirements, and trainer type.

Maintenance requests a list when it has no list for that trainer. It then selects the highest-level eligible unknown class spell, breaking ties by lower cost and spell ID. It preserves 1,000 copper and sends one typed buy action. The shared action validator rechecks the current trainer entity, range, offer, player level, known spell set, required skill, and money before dispatch. The action does not add a spell to local state; the server's normal learned-spell observation does that.

The increment does not add remembered service locations or travel. This keeps class training grounded in a live nearby NPC and leaves trainer discovery and routing for a later service-planning increment.
