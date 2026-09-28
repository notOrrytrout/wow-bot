use wow_state::entities::EntityState;

pub fn normalized_name(value: &str) -> String {
    wow_domain::text::normalize_name(value)
}

pub fn matches_resource(entity: &EntityState, resource: &str) -> bool {
    entity
        .name
        .as_deref()
        .is_some_and(|name| normalized_name(name) == normalized_name(resource))
}
