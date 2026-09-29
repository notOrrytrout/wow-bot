# Design

The policy checks active talent ID 1984 and known spell 46584 for a Death Knight. For a Mage, it resolves active glyph property IDs through the embedded build-12340 GlyphProperties catalog and requires the Eternal Water effect spell before it accepts known spell 31687. Both paths check pet state, retry state, and shared readiness. They return `SummonPet`, which the lane validates and sends through the existing summon path. The retry classifier treats both spells as persistent pet summons so failure handling uses the existing bounded pet retry window.
