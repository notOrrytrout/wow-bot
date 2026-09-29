use wow_domain::EntityId;
use wow_state::{Snapshot, entities::EntityKind};

/// Return the one trusted AzerothCore mailbox within interaction range.
/// Ambiguous, distant, cross-map, or unknown objects remain unavailable.
pub(crate) fn unique_nearby_mailbox(snapshot: &Snapshot) -> Option<EntityId> {
    let player = snapshot
        .state
        .control
        .active_position(snapshot.state.position.player)?;
    let catalog = wow_infra::world_knowledge::embedded_azerothcore_catalog();
    let mut matches = snapshot.state.entities.0.values().filter(|entity| {
        entity.kind == EntityKind::GameObject
            && entity.interactable
            && catalog
                .gameobject_name(entity.entry)
                .is_some_and(|name| name.to_ascii_lowercase().contains("mailbox"))
            && entity.position.is_some_and(|position| {
                position.map == player.map && position.point.distance(player.point) <= 5.0
            })
    });
    let mailbox = matches.next()?.id;
    matches.next().is_none().then_some(mailbox)
}
