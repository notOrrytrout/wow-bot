use wow_domain::EntityId;
use wow_state::Snapshot;
pub fn vendor_interaction_valid(state: &Snapshot, vendor: EntityId) -> bool {
    state.state.inventory.vendor == Some(vendor)
        && state
            .state
            .entities
            .0
            .get(&vendor)
            .is_some_and(|e| e.interactable)
}
