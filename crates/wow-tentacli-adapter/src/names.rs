use tentacli::plugins::wow::wotlk::realm::object::{Object, ObjectNameRegistry};
pub fn normalized_name(name: &str) -> String {
    name.trim().to_lowercase()
}
pub fn object_name<'a>(registry: &'a ObjectNameRegistry, object: &Object) -> Option<&'a str> {
    registry.name_for(object)
}
