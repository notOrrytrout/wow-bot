use wow_domain::EntityId;
use wow_state::Snapshot;

/// A server-observed group loot method. Unknown values must stay `Unknown` so
/// policy cannot treat master loot or an unsupported method as a roll method.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupLootMethod {
    Unknown,
    FreeForAll,
    RoundRobin,
    MasterLoot,
    GroupLoot,
    NeedBeforeGreed,
}

/// The choices currently allowed by one authoritative roll request.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LootRollChoices {
    pub pass: bool,
    pub need: bool,
    pub greed: bool,
    pub disenchant: bool,
}

/// A live request parsed from the server's roll-start message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LootRollRequest {
    pub item: EntityId,
    pub item_id: u32,
    pub expires_at_ms: u64,
    pub choices: LootRollChoices,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LootEligibility {
    Unknown,
    Ineligible,
    Eligible,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LootRollVote {
    Pass,
    Need,
    Greed,
    Disenchant,
}

/// Per-operator limits for automatic group loot rolls.
///
/// Defaults pass every roll. A non-pass choice requires an explicit setting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroupLootPolicy {
    pub need_usable_upgrades: bool,
    pub greed_non_upgrades: bool,
    pub disenchant_non_upgrades: bool,
    pub max_need_quality: u32,
}

impl Default for GroupLootPolicy {
    fn default() -> Self {
        Self {
            need_usable_upgrades: false,
            greed_non_upgrades: false,
            disenchant_non_upgrades: false,
            max_need_quality: 4,
        }
    }
}

/// Select a vote only from a live eligible request and a roll-based loot
/// method. Unknown eligibility or incomplete upgrade evidence never permits
/// need, greed, or disenchant. When enabled, configured choices are considered
/// in upgrade-first order, then the server-allowed pass choice.
pub fn group_loot_vote(
    method: GroupLootMethod,
    request: Option<&LootRollRequest>,
    now_ms: u64,
    eligibility: LootEligibility,
    usable_upgrade: Option<bool>,
    quality: Option<u32>,
    policy: GroupLootPolicy,
) -> Option<LootRollVote> {
    let request = request?;
    if now_ms >= request.expires_at_ms
        || eligibility != LootEligibility::Eligible
        || !matches!(
            method,
            GroupLootMethod::GroupLoot | GroupLootMethod::NeedBeforeGreed
        )
    {
        return None;
    }

    let choices = request.choices;
    if policy.need_usable_upgrades
        && usable_upgrade == Some(true)
        && quality.is_some_and(|quality| quality <= policy.max_need_quality)
        && choices.need
    {
        return Some(LootRollVote::Need);
    }
    if policy.disenchant_non_upgrades && usable_upgrade == Some(false) && choices.disenchant {
        return Some(LootRollVote::Disenchant);
    }
    if policy.greed_non_upgrades && usable_upgrade == Some(false) && choices.greed {
        return Some(LootRollVote::Greed);
    }
    choices.pass.then_some(LootRollVote::Pass)
}

#[derive(Clone, Copy, Debug)]
pub struct LootIntent {
    pub target: EntityId,
    pub generation: u64,
}
pub fn still_current(state: &Snapshot, intent: LootIntent) -> bool {
    state.state.inventory.current_loot == Some(intent.target)
        && state.state.inventory.loot_generation == intent.generation
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> LootRollRequest {
        LootRollRequest {
            item: EntityId(7),
            item_id: 123,
            expires_at_ms: 100,
            choices: LootRollChoices {
                pass: true,
                need: true,
                greed: true,
                disenchant: true,
            },
        }
    }

    #[test]
    fn group_loot_defaults_to_passing_a_live_eligible_roll() {
        assert_eq!(
            group_loot_vote(
                GroupLootMethod::NeedBeforeGreed,
                Some(&request()),
                0,
                LootEligibility::Eligible,
                Some(true),
                Some(3),
                GroupLootPolicy::default(),
            ),
            Some(LootRollVote::Pass)
        );
    }

    #[test]
    fn need_requires_a_configured_usable_upgrade_and_allowed_choice() {
        let policy = GroupLootPolicy {
            need_usable_upgrades: true,
            ..GroupLootPolicy::default()
        };
        let request = request();
        assert_eq!(
            group_loot_vote(
                GroupLootMethod::NeedBeforeGreed,
                Some(&request),
                0,
                LootEligibility::Eligible,
                Some(true),
                Some(3),
                policy,
            ),
            Some(LootRollVote::Need)
        );
        assert_eq!(
            group_loot_vote(
                GroupLootMethod::NeedBeforeGreed,
                Some(&request),
                0,
                LootEligibility::Eligible,
                Some(true),
                None,
                policy,
            ),
            Some(LootRollVote::Pass)
        );
        assert_eq!(
            group_loot_vote(
                GroupLootMethod::NeedBeforeGreed,
                Some(&request),
                0,
                LootEligibility::Eligible,
                Some(true),
                Some(3),
                GroupLootPolicy {
                    max_need_quality: 2,
                    ..policy
                },
            ),
            Some(LootRollVote::Pass)
        );
        assert_eq!(
            group_loot_vote(
                GroupLootMethod::NeedBeforeGreed,
                Some(&LootRollRequest {
                    choices: LootRollChoices {
                        need: false,
                        ..request.choices
                    },
                    ..request
                }),
                0,
                LootEligibility::Eligible,
                Some(true),
                Some(3),
                policy,
            ),
            Some(LootRollVote::Pass)
        );
    }

    #[test]
    fn unknown_or_ineligible_rolls_and_non_roll_methods_are_not_voted() {
        let request = request();
        for (method, eligibility, request) in [
            (
                GroupLootMethod::NeedBeforeGreed,
                LootEligibility::Unknown,
                Some(&request),
            ),
            (
                GroupLootMethod::GroupLoot,
                LootEligibility::Ineligible,
                Some(&request),
            ),
            (
                GroupLootMethod::MasterLoot,
                LootEligibility::Eligible,
                Some(&request),
            ),
            (
                GroupLootMethod::Unknown,
                LootEligibility::Eligible,
                Some(&request),
            ),
            (GroupLootMethod::GroupLoot, LootEligibility::Eligible, None),
        ] {
            assert_eq!(
                group_loot_vote(
                    method,
                    request,
                    0,
                    eligibility,
                    Some(false),
                    Some(3),
                    GroupLootPolicy {
                        greed_non_upgrades: true,
                        ..GroupLootPolicy::default()
                    },
                ),
                None
            );
        }
        assert_eq!(
            group_loot_vote(
                GroupLootMethod::NeedBeforeGreed,
                Some(&request),
                request.expires_at_ms,
                LootEligibility::Eligible,
                Some(true),
                Some(3),
                GroupLootPolicy {
                    need_usable_upgrades: true,
                    ..GroupLootPolicy::default()
                },
            ),
            None
        );
    }

    #[test]
    fn non_upgrade_rolls_require_known_non_upgrade_evidence() {
        let request = request();
        let policy = GroupLootPolicy {
            greed_non_upgrades: true,
            disenchant_non_upgrades: true,
            ..GroupLootPolicy::default()
        };
        assert_eq!(
            group_loot_vote(
                GroupLootMethod::GroupLoot,
                Some(&request),
                0,
                LootEligibility::Eligible,
                None,
                None,
                policy,
            ),
            Some(LootRollVote::Pass)
        );
        assert_eq!(
            group_loot_vote(
                GroupLootMethod::GroupLoot,
                Some(&request),
                0,
                LootEligibility::Eligible,
                Some(false),
                Some(2),
                policy,
            ),
            Some(LootRollVote::Disenchant)
        );
        assert_eq!(
            group_loot_vote(
                GroupLootMethod::GroupLoot,
                Some(&LootRollRequest {
                    choices: LootRollChoices {
                        disenchant: false,
                        ..request.choices
                    },
                    ..request
                }),
                0,
                LootEligibility::Eligible,
                Some(false),
                Some(2),
                policy,
            ),
            Some(LootRollVote::Greed)
        );
    }
}
