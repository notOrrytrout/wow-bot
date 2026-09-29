# Integrate Legacy Group Follow Spacing

The old bot used different follow distances for each group role. The new lane runtime currently follows a group member at one fixed distance. Use the configured Party or Raid role to choose the follow gap and calculate the destination in three dimensions.

This reuses the new bot's observed group state and movement controller. It does not add a separate legacy movement path.

## Scope

- Apply the legacy role distances to Party and Raid missions.
- Keep following limited to observed online members on the same map.
- Verify role mapping and three-dimensional spacing with focused tests.
