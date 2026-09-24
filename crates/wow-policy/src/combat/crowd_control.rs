use std::collections::BTreeSet;
use wow_domain::EntityId;
#[derive(Clone, Debug, Default)]
pub struct CrowdControlState {
    pub controlled: BTreeSet<EntityId>,
}
pub fn preserves_cc(target: EntityId, aoe_targets: &[EntityId], cc: &CrowdControlState) -> bool {
    !cc.controlled.contains(&target) && !aoe_targets.iter().any(|e| cc.controlled.contains(e))
}
