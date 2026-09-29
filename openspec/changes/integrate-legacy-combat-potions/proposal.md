# Integrate Legacy Combat Potions

Restore the old bot's critical health- and mana-potion behavior in the lane combat path. Use authoritative backpack instances and item templates, respect Potion Sickness, and share a one-hour fallback lockout across both potion types. Extend opportunistic vendor maintenance to keep up to three health and mana potions when the character is already beside a trusted seller.

Potion restocking uses current offer data, reviewed item-name cues, class and level eligibility, stock, a single vendor lot, and the existing 1,000-copper reserve. It does not cause vendor travel and does not replace the existing food, drink, bandage, healthstone, or class-supply behavior.
