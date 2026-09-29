# Design

Maintenance first keeps its existing priority for equipping an owned inventory upgrade. It then considers a vendor gear upgrade only when the lane passes a current nearby cataloged seller. It never creates travel for this optional work.

The policy requires authoritative inventory instances and equipment slots, at least one free bag slot, a known player class and level, and complete metadata for equipped items. It skips the purchase when the player is in combat, is attached to a transport, or has a nearby quest offer within 40 yards. If the current vendor offer list is missing, maintenance requests it. It queries missing metadata only for an offer that fits the wallet budget, has no extended cost, and has enough stock for one lot or server-reported unlimited stock.

Candidate items must be usable by the player and improve the best valid equipment slot by at least 5 percent using the shared gear score policy. Ranking uses the largest score gain, then lower price and lower item ID. Exact ties keep the order from the authoritative vendor list.

The offer price must fit both a 25 percent wallet cap and the existing 1,000-copper maintenance reserve. Purchases use one observed lot. Final action validation continues to bind the current vendor, slot, item, stock, metadata, seller, and money reserve. The lane applies the existing bounded vendor purchase retry after sending a buy request.

The old policy reserves 100 copper and caps spending at 25 percent of the wallet. This port preserves the percentage cap and applies the lane's stricter 1,000-copper reserve at both selection and final validation.
