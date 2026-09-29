use super::{ActionValidator, ValidationContext};
use wow_domain::*;
use wow_state::Snapshot;

pub fn finalize(
    snapshot: &Snapshot,
    current: ValidityStamp,
    stage: ActivationStage,
    permissions: PermissionSet,
    bank_keep_item_ids: &[u32],
    auto_professions_enabled: bool,
    battleground_pvp_authorized: bool,
    fresh_group_loot_rolls: &[(EntityId, u32)],
    action: ProposedAction,
) -> ValidationOutcome {
    ActionValidator::validate(
        snapshot,
        ValidationContext {
            current,
            stage,
            permissions,
            bank_keep_item_ids: bank_keep_item_ids.to_vec(),
            auto_professions_enabled,
            battleground_pvp_authorized,
            fresh_group_loot_rolls: fresh_group_loot_rolls.to_vec(),
        },
        action,
    )
}
