use wow_state::Snapshot;

#[derive(Clone, Debug)]
pub struct RecipeRequirement { pub recipe: u32, pub skill: Option<(u32, u16)>, pub reagents: Vec<(u32, u32)> }

pub fn can_craft(state: &Snapshot, req: &RecipeRequirement) -> bool {
    if !state.state.professions.known_recipes.contains(&req.recipe) { return false; }
    if let Some((skill, required)) = req.skill {
        if state.state.professions.skill(skill) < required { return false; }
    }
    req.reagents.iter().all(|(item, count)| state.state.inventory.has(*item, *count))
}
