# Integrate safe bank deposits under bag pressure

The old bot frees backpack space by depositing known profession materials at a nearby banker when no more than two backpack slots remain. Add this behavior to the lane runtime with server-confirmed bank state, strict item exclusions, and final action validation.

This increment uses only an already observed banker within interaction range. It does not add remembered-bank travel or item withdrawals.
