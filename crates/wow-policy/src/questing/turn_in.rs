use wow_state::Snapshot;pub fn eligible(state:&Snapshot,quest:u32)->bool{state.state.quests.active.get(&quest).is_some_and(|q|q.complete)}
