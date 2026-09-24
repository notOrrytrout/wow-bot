use crate::capabilities::TalentRank;
use serde::Deserialize;
use std::{collections::BTreeMap, sync::OnceLock};

#[derive(Clone, Copy, Debug, Deserialize)]
struct TalentLocation {
    class_id: u8,
    tree: u8,
}

#[derive(Deserialize)]
struct TalentTreeCatalog {
    format_version: u32,
    talents: BTreeMap<u32, TalentLocation>,
}

static TALENT_TREES: OnceLock<TalentTreeCatalog> = OnceLock::new();

fn catalog() -> &'static TalentTreeCatalog {
    TALENT_TREES.get_or_init(|| {
        let catalog: TalentTreeCatalog =
            serde_json::from_str(include_str!("../data/wotlk-talent-trees.json"))
                .expect("generated WotLK talent tree catalog");
        assert_eq!(catalog.format_version, 1);
        catalog
    })
}

/// Resolve the active talent tree only when every selected talent has reviewed metadata
/// and one tree has a unique highest point count.
pub fn active_tree(class_id: Option<u8>, talents: &[TalentRank]) -> Option<u8> {
    let class_id = class_id?;
    if talents.is_empty() {
        return None;
    }
    let definitions = &catalog().talents;
    let mut points = [0u32; 3];
    for talent in talents {
        let location = definitions.get(&talent.talent_id)?;
        if location.class_id != class_id || location.tree >= 3 || talent.rank >= 5 {
            return None;
        }
        points[location.tree as usize] += u32::from(talent.rank) + 1;
    }
    let highest = *points.iter().max()?;
    if highest == 0 || points.iter().filter(|points| **points == highest).count() != 1 {
        return None;
    }
    points
        .iter()
        .position(|points| *points == highest)
        .map(|tree| tree as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_a_unique_tree_and_rejects_unknown_or_ambiguous_data() {
        assert_eq!(
            active_tree(
                Some(8),
                &[
                    TalentRank {
                        talent_id: 74,
                        rank: 2,
                    },
                    TalentRank {
                        talent_id: 27,
                        rank: 0,
                    },
                ]
            ),
            Some(0)
        );
        assert_eq!(
            active_tree(
                Some(8),
                &[
                    TalentRank {
                        talent_id: 74,
                        rank: 0,
                    },
                    TalentRank {
                        talent_id: 27,
                        rank: 0,
                    },
                ]
            ),
            None
        );
        assert_eq!(
            active_tree(
                Some(8),
                &[TalentRank {
                    talent_id: u32::MAX,
                    rank: 0,
                }]
            ),
            None
        );
        assert_eq!(
            active_tree(
                Some(5),
                &[TalentRank {
                    talent_id: 74,
                    rank: 4,
                }]
            ),
            None
        );
    }
}
