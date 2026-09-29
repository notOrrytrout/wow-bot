# Integrate opportunistic vendor gear upgrades

The old bot can inspect a vendor that is already within interaction range and buy a usable gear upgrade. The lane runtime already observes vendor offers and validates vendor purchases, but maintenance does not select gear offers.

Add a deterministic maintenance policy for safe gear upgrades. The policy must preserve the lane runtime's authoritative state, final action validation, one-lot purchase bound, and no-travel behavior.
