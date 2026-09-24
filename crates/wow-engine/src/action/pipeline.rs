use super::{ActionValidator, ValidationContext};
use wow_domain::*;
use wow_state::Snapshot;

pub fn finalize(
    snapshot: &Snapshot,
    current: ValidityStamp,
    stage: ActivationStage,
    permissions: PermissionSet,
    action: ProposedAction,
) -> ValidationOutcome {
    ActionValidator::validate(
        snapshot,
        ValidationContext {
            current,
            stage,
            permissions,
        },
        action,
    )
}
