# Design

The maintenance policy shares food and drink item classification with Mage supply maintenance. It counts only backpack instances, and a refreshment can count as both food and drink. It classifies bandages from item class, subclass, and use-spell metadata. If any occupied backpack instance lacks item metadata, it waits rather than assume that a recovery reserve is low. It considers bandages a restock target when an eligible bandage is already observed in inventory or on the current vendor list.

The lane passes only its existing nearby cataloged seller into maintenance. Policy decisions use that seller's current offer list, authoritative stock and lot size, fixed copper price, item class and level eligibility, and the shared 1,000-copper reserve. Purchases use the existing typed vendor-buy command, so final action validation still checks the offer identity and lot bound.

The engine stores the purchased item count as a baseline and blocks another recovery purchase until authoritative inventory reports a different count for that item. A 30-second deadline releases the pending state if the purchase fails or inventory never updates. Leaving the world clears the pending state.
