use wow_domain::EntityId;
#[derive(Clone, Debug, Default)]
pub struct PetState {
    pub pet: Option<EntityId>,
    pub target: Option<EntityId>,
}
pub fn grants_encounter_authority(_: &PetState) -> bool {
    false
}
