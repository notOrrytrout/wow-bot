# Integrate Legacy Persistent Companion Maintenance

The old bot restored a Death Knight ghoul only when Master of Ghouls was active and Raise Dead was known. It also restored a Mage Water Elemental only when Eternal Water was active. The new bot observed talents, spells, and pet state but did not restore these persistent companions.

Add class policy branches to shared lane maintenance. Keep the talent, glyph, and known spell gates, use shared spell readiness, and dispatch through the existing class pet summon action.
