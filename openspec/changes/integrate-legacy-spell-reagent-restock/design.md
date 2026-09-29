# Design

The policy scans metadata only for spells in the authoritative known-spell set. The current spell catalogue has no `self_buff` field, which the old bot used as one automated-utility signal. This policy uses Spell.dbc power-cost fields (including percent costs), `attack_spell`, and the same reviewed class-utility name families instead. The current catalogue identifies relevant reagent-bearing class self-buffs through their power costs (for example, Slow Fall has a percent cost), while the reviewed utility names cover the listed zero-cost utility spells. Unknown zero-cost spells are excluded. This keeps uncertain class summons and profession recipes out of the reserve; adding broader classification requires explicit metadata support.

The policy collapses reagent requirements by item and retains the largest per-cast amount so a lower rank cannot reduce the reserve.

The policy uses authoritative aggregate inventory counts and the lane's already-nearby trusted vendor. It requests current vendor data when needed, then chooses the cheapest matching offer with fixed copper cost, sufficient stock, and a price that leaves the shared maintenance reserve. Purchases use the existing bounded `RecoveryVendorBuy` action and its inventory-change timeout. Soul Shards are excluded because the old bot creates them through combat. No travel or new transport behavior is added.
