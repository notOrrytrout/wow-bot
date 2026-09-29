use serde::{Deserialize, Serialize};
use wow_domain::EntityId;

/// Group loot settings received in the authoritative group roster packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum GroupLootMethod {
    Unknown(u8),
    FreeForAll,
    RoundRobin,
    MasterLoot,
    GroupLoot,
    NeedBeforeGreed,
}

impl GroupLootMethod {
    pub const fn from_wire(value: u8) -> Self {
        match value {
            0 => Self::FreeForAll,
            1 => Self::RoundRobin,
            2 => Self::MasterLoot,
            3 => Self::GroupLoot,
            4 => Self::NeedBeforeGreed,
            value => Self::Unknown(value),
        }
    }
}

/// A live request from SMSG_LOOT_START_ROLL.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GroupLootRollRequest {
    pub item: EntityId,
    pub map_id: u32,
    pub item_slot: u32,
    pub item_id: u32,
    pub item_count: u32,
    pub countdown_ms: u32,
    pub vote_mask: u8,
}

impl GroupLootRollRequest {
    pub const fn allows(&self, vote: u8) -> bool {
        vote < 8 && self.vote_mask & (1 << vote) != 0
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GroupMember {
    pub entity: EntityId,
    pub name: String,
    pub role: Option<String>,
    pub online: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum GroupLifecycle {
    #[default]
    Solo,
    Forming,
    Active,
    Leaving,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GroupState {
    pub generation: u64,
    pub members: Vec<GroupMember>,
    pub leader: Option<EntityId>,
    pub raid: bool,
    pub lifecycle: GroupLifecycle,
    pub encounter_target: Option<EntityId>,
    #[serde(default)]
    pub loot_method: Option<GroupLootMethod>,
    #[serde(default)]
    pub loot_rolls: Vec<GroupLootRollRequest>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loot_votes_use_the_observed_allowed_choice_mask() {
        let roll = GroupLootRollRequest {
            item: EntityId(1),
            map_id: 571,
            item_slot: 2,
            item_id: 123,
            item_count: 1,
            countdown_ms: 30_000,
            vote_mask: 0b1011,
        };
        assert!(roll.allows(0));
        assert!(roll.allows(1));
        assert!(!roll.allows(2));
        assert!(roll.allows(3));
        assert!(!roll.allows(8));
    }

    #[test]
    fn reducer_tracks_group_method_and_replaces_duplicate_live_rolls() {
        let mut state = crate::AuthoritativeState::default();
        crate::reduce(
            &mut state,
            crate::ProtocolObservation::GroupLootMethod(Some(GroupLootMethod::NeedBeforeGreed)),
        );
        let request = GroupLootRollRequest {
            item: EntityId(9),
            map_id: 571,
            item_slot: 1,
            item_id: 100,
            item_count: 1,
            countdown_ms: 30_000,
            vote_mask: 1,
        };
        crate::reduce(
            &mut state,
            crate::ProtocolObservation::GroupLootRollStarted(request.clone()),
        );
        let mut updated = request.clone();
        updated.item_id = 101;
        crate::reduce(
            &mut state,
            crate::ProtocolObservation::GroupLootRollStarted(updated.clone()),
        );

        assert_eq!(
            state.group.loot_method,
            Some(GroupLootMethod::NeedBeforeGreed)
        );
        assert_eq!(state.group.loot_rolls, vec![updated]);
        crate::reduce(
            &mut state,
            crate::ProtocolObservation::GroupLootMethod(None),
        );
        assert_eq!(state.group.loot_method, None);
    }
}
