# Integrate Legacy Equipment Repair

The old bot watched equipped-item durability and repaired broken gear or gear below the yellow durability threshold. The new lane had item equipment state and vendor travel, but it did not observe durability or send repair requests.

Add durability observations and a lane maintenance repair path. Reuse Tentacli's packet-derived equipment fields, trusted vendor service data, existing navigation, action validation, and the current proxy packet encoder.
