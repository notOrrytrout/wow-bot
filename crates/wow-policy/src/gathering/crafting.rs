use wow_state::Snapshot;

#[derive(Clone, Debug)]
pub struct RecipeRequirement {
    pub recipe: u32,
    pub skill: Option<(u32, u16)>,
    pub reagents: Vec<(u32, u32)>,
}

pub fn can_craft(state: &Snapshot, req: &RecipeRequirement) -> bool {
    if !state.state.professions.known {
        return false;
    }
    if !state.state.professions.known_recipes.contains(&req.recipe) {
        return false;
    }
    if let Some((skill, required)) = req.skill {
        if state.state.professions.skill(skill) < required {
            return false;
        }
    }
    req.reagents
        .iter()
        .all(|(item, count)| state.state.inventory.has(*item, *count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_state::{AuthoritativeState, Snapshot};

    #[test]
    fn crafting_requires_known_recipe_sufficient_rank_and_all_materials() {
        let mut state = Snapshot::from_state(&AuthoritativeState::default());
        let req = RecipeRequirement {
            recipe: 44,
            skill: Some((164, 20)),
            reagents: vec![(100, 2)],
        };
        assert!(!can_craft(&state, &req));
        state.state.professions.known = true;
        state.state.professions.known_recipes.insert(44);
        state.state.professions.skills.insert(164, (19, 75));
        state.state.inventory.items.insert(100, 2);
        assert!(!can_craft(&state, &req));
        state.state.professions.skills.insert(164, (20, 75));
        assert!(can_craft(&state, &req));
        state.state.inventory.items.insert(100, 1);
        assert!(!can_craft(&state, &req));
    }
}
