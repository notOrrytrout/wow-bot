use wow_domain::{EntityId, TaskId, Vec3};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct QuestWorkId(pub u64);

#[derive(Clone, Debug, PartialEq)]
pub enum QuestWorkKey {
    AcquireQuest {
        quest: Option<u32>,
    },
    QueryDefinition {
        quest: u32,
    },
    TravelToObjective {
        quest: u32,
        objective: usize,
        destination: Vec3,
    },
    CombatObjective {
        quest: u32,
        objective: usize,
        target: EntityId,
    },
    InteractObjective {
        quest: u32,
        objective: usize,
        target: EntityId,
    },
    CollectItem {
        quest: u32,
        item: u32,
    },
    TurnIn {
        quest: u32,
    },
}

#[derive(Clone, Debug)]
pub struct QuestWorkRuntime {
    pub id: QuestWorkId,
    pub key: QuestWorkKey,
}

#[derive(Clone, Debug)]
pub enum QuestChildWork {
    Travel { task: TaskId, destination: Vec3 },
    Combat { task: TaskId, target: EntityId },
    Gather { task: TaskId, item: u32 },
    Interact { task: TaskId, target: EntityId },
}
