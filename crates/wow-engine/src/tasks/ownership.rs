use wow_domain::{ActivityGeneration, TaskId};
#[derive(Clone, Copy, Debug)]
pub struct TaskOwnership {
    pub task: TaskId,
    pub generation: ActivityGeneration,
}
