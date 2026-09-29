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
        },
        action,
    )
}
