use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use wow_domain::{AccountId, LaneId};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RosterEntry {
    pub lane: LaneId,
    pub account: AccountId,
    pub account_name: String,
    pub character: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Roster {
    pub entries: Vec<RosterEntry>,
}

impl Roster {
    pub fn select<'a>(&'a self, lane_names: &[LaneId]) -> Result<Vec<&'a RosterEntry>, String> {
        let wanted: BTreeSet<_> = lane_names.iter().copied().collect();
        let selected: Vec<_> = self
            .entries
            .iter()
            .filter(|e| e.enabled && wanted.contains(&e.lane))
            .collect();
        if selected.len() != wanted.len() {
            return Err("one or more selected lanes are missing or disabled".into());
        }
        Ok(selected)
    }
}
