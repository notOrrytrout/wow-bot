use super::ActivityKind;
use wow_domain::{ActivityGeneration, ActivityId, TaskId};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActivityLease {
    pub id: ActivityId,
    pub owner: TaskId,
    pub kind: ActivityKind,
    pub generation: ActivityGeneration,
}
