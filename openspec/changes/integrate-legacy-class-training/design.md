# Design

The lane selects only a currently observed trainer NPC within five yards. Current NPC service flags identify trainer units. The server's trainer-list response is the authority for offer usability, cost, level, skill requirements, and trainer type.

Maintenance requests a list when it has no list for that trainer. It then selects the highest-level eligible unknown class spell, breaking ties by lower cost and spell ID. It preserves 1,000 copper and sends one typed buy action. The shared action validator rechecks the current trainer entity, range, offer, player level, known spell set, required skill, and money before dispatch. The action does not add a spell to local state; the server's normal learned-spell observation does that.

The lane remembers only positions from live, observed class-trainer entities, keyed by map, for up to six hours. When an authoritative player level observation initializes or increases the level, class training becomes due. After nearby maintenance has no local trainer action, the lane may route to the remembered trainer only when the target is on the current map and within a 30-yard detour. It does not use static trainer data or plan cross-map travel.

The trip requires both maintenance and movement permission, solo group state, no authoritative combat, more than the 1,000-copper reserve, and no observed quest offer within 40 yards. It leaves the active mission intent unchanged. A newly visible local class trainer or a failed gate cancels the detour. If the trainer is no longer visible at the remembered point, a five-minute retry bound prevents repeated travel attempts. A class trainer list with no eligible unknown spell clears the training need; eligible but unaffordable offers delay another trip for five minutes.
