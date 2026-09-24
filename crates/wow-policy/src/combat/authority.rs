use wow_domain::EntityId;
use wow_state::Snapshot;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OffensiveAuthority {
    None,
    VoluntaryMission,
    SelfDefense,
    Battleground,
    GroupEncounter,
}
pub fn may_attack(state: &Snapshot, target: EntityId, authority: OffensiveAuthority) -> bool {
    authority != OffensiveAuthority::None
        && state
            .state
            .entities
            .0
            .get(&target)
            .is_some_and(|e| e.hostile)
}
