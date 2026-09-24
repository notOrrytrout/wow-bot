use wow_domain::WorldPosition;
use wow_infra::world_knowledge::AzerothCoreCatalog;
use wow_state::Snapshot;

/// Supported primary skill lines. These IDs come from the WotLK SkillLine table.
pub const PRIMARY_SKILLS: &[u32] = &[164, 165, 171, 182, 186, 197, 202, 333, 393, 755, 773];
pub const COOKING: u32 = 185;
pub const FIRST_AID: u32 = 129;
pub const FISHING: u32 = 356;

pub fn learned(state: &Snapshot, skill: u32) -> bool {
    state.state.professions.skills.contains_key(&skill)
}

/// Return preserved primary lines and fill free slots with gathering-compatible
/// profession goals. Unknown profession state must not be treated as empty.
pub fn selected_skills(state: &Snapshot) -> Option<Vec<u32>> {
    let professions = &state.state.professions;
    if !professions.known {
        return None;
    }
    let mut selected = PRIMARY_SKILLS
        .iter()
        .copied()
        .filter(|skill| professions.skill(*skill) > 0)
        .take(2)
        .collect::<Vec<_>>();
    let partner = selected.first().and_then(|skill| match skill {
        164 | 202 | 755 => Some(186),
        165 => Some(393),
        171 | 773 => Some(182),
        186 => Some(164),
        393 => Some(165),
        182 => Some(171),
        197 => Some(333),
        _ => None,
    });
    for skill in partner.into_iter().chain([182, 171]) {
        if selected.len() == 2 {
            break;
        }
        if !selected.contains(&skill) {
            selected.push(skill);
        }
    }
    selected.extend([FIRST_AID, COOKING, FISHING]);
    Some(selected)
}

/// Return a location for a trainer for an unlearned selected profession.
/// The result is advisory: movement and interaction still require normal action
/// validation and live target authority.
pub fn trainer_destination(
    state: &Snapshot,
    catalog: &AzerothCoreCatalog,
) -> Option<(u32, WorldPosition)> {
    if !state.state.professions.known {
        return None;
    }
    let pos = state.state.position.player?;
    let (entry, point) = selected_skills(state)?
        .into_iter()
        .filter(|skill| !learned(state, *skill))
        .filter_map(|skill| {
            catalog
                .nearest_profession_trainer(skill, pos.map, pos.point)
                .map(|(entry, point)| (entry, point))
        })
        .min_by(|a, b| pos.point.distance(a.1).total_cmp(&pos.point.distance(b.1)))?;
    Some((
        entry,
        WorldPosition {
            map: pos.map,
            point,
            orientation: 0.0,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_state::{AuthoritativeState, Snapshot};

    #[test]
    fn profession_selection_requires_known_ranks_and_preserves_known_lines() {
        let mut state = Snapshot::from_state(&AuthoritativeState::default());
        assert_eq!(selected_skills(&state), None);
        state.state.professions.known = true;
        state.state.professions.skills.insert(164, (75, 150));
        assert_eq!(selected_skills(&state).unwrap()[..2], [164, 186]);
    }

    #[test]
    fn trainer_is_unavailable_without_known_ranks_position_or_matching_catalog() {
        let mut state = Snapshot::from_state(&AuthoritativeState::default());
        state.state.professions.known = true;
        let catalog = wow_infra::world_knowledge::embedded_azerothcore_catalog();
        assert!(trainer_destination(&state, catalog).is_none());
        state.state.position.player = Some(WorldPosition {
            map: 0,
            point: wow_domain::Vec3::default(),
            orientation: 0.0,
        });
        assert!(trainer_destination(&state, catalog).is_some());
    }
}
