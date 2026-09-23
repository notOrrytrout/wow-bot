use wow_domain::EntityId;use wow_state::Snapshot;
#[derive(Clone,Copy,Debug)]pub struct LootIntent{pub target:EntityId,pub generation:u64}
pub fn still_current(state:&Snapshot,intent:LootIntent)->bool{state.state.inventory.current_loot==Some(intent.target)&&state.state.inventory.loot_generation==intent.generation}
