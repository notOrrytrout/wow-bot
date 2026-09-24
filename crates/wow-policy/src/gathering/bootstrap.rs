use std::collections::BTreeSet;

#[derive(Clone, Debug, Default)]
pub struct BootstrapPlan {
    pub preserve_primaries: BTreeSet<u32>,
    pub learn_cooking: bool,
    pub learn_first_aid_if_available: bool,
}

pub fn default_plan(
    existing_primaries: &BTreeSet<u32>,
    cooking_known: bool,
    first_aid_known: bool,
) -> BootstrapPlan {
    BootstrapPlan {
        preserve_primaries: existing_primaries.clone(),
        learn_cooking: !cooking_known,
        learn_first_aid_if_available: !first_aid_known,
    }
}
