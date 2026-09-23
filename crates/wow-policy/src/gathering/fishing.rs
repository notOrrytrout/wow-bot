use wow_domain::EntityId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FishingStage { EquipPole, Cast, WaitBobber, UseBobber, Loot, Done }

#[derive(Clone, Debug)]
pub struct FishingAttempt {
    pub cast_generation: u64,
    pub player: EntityId,
    pub bobber: Option<EntityId>,
    pub stage: FishingStage,
}

impl FishingAttempt {
    pub fn accepts_bobber(&self, observed_cast_generation: u64, owner: EntityId) -> bool {
        observed_cast_generation == self.cast_generation && owner == self.player && self.stage == FishingStage::WaitBobber
    }
}
