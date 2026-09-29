# Design

Ammo maintenance runs in the lane maintenance policy. It requires an authoritative equipment-slot observation for ranged slot 17, the ranged weapon template, an authoritative inventory-instance snapshot, and known templates for occupied backpack items. Ammo must use item class 6, match the weapon's AmmoType subclass, meet the player's required level, and allow the player's class.

The reserve uses integer shots per minute derived from weapon delay. The minimum is five minutes of shots and the purchase target is fifteen minutes. The lane passes only its existing nearby cataloged seller. The policy uses the current vendor response, fixed copper price, sufficient stock for one lot, and the shared 1,000-copper reserve. It reuses the existing one-lot vendor validation and pending inventory-count confirmation path. No travel or Auction House purchase is added.

The current protocol observations do not expose which ammo entry is selected. The maintenance lane therefore sends the typed SetAmmo action only after a complete authoritative inventory snapshot proves a compatible stack exists, and limits repeat selection requests to once per item per hour. The proxy addition encodes only CMSG_SET_AMMO.
