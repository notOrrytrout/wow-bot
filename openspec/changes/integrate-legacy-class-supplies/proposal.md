# Integrate Legacy Mage Supplies

The old bot kept a reserve of food and water for Mages. The new bot has authoritative item templates and a shared maintenance cast path, but it did not restore those supplies.

Add Mage and Warlock supply checks to lane maintenance. Count Mage food and drink from backpack item templates, wait while any backpack template is unknown, and cast a known ready Conjure spell when a reserve is low. Count refreshment items toward both reserves. For Warlocks, create missing stones and apply an unused Soulstone through typed maintenance work.
