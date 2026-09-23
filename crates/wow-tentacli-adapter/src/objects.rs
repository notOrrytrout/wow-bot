use tentacli::plugins::wow::wotlk::realm::object::{Object, ObjectNameRegistry, ObjectTypeId, types::update_fields::{FieldValue, UnitField}};
use wow_domain::{EntityId, Vec3, WorldPosition};
use wow_state::{entities::{EntityKind, EntityState}, ProtocolObservation};

pub fn object_to_entity(object: &Object, names: Option<&ObjectNameRegistry>, map_id: u32) -> EntityState {
    let id = EntityId(object.guid().0);
    let entry = object.entry_id().unwrap_or_default();
    let position = object.position().map(|point| WorldPosition {
        map: map_id,
        point: Vec3::new(point.x, point.y, point.z),
        orientation: object.facing().unwrap_or_default(),
    });
    let health = object.as_unit().and_then(|unit| unit.health().zip(unit.max_health()));
    let power = object.as_unit().and_then(|unit| unit.power().zip(unit.max_power()));
    let power_type = object.as_unit().and_then(|unit| unit.power_type()).map(u8::from);
    let target = match object.unit_fields.get(&UnitField::Target) {
        Some(FieldValue::Long(value)) if *value != 0 => Some(EntityId(*value)),
        _ => None,
    };
    let interactable = match object.object_type_id() {
        ObjectTypeId::GameObject => true,
        ObjectTypeId::Unit | ObjectTypeId::Player => object.as_unit().and_then(|unit| unit.npc_flags()).unwrap_or_default() != 0,
        _ => false,
    };
    let kind = match object.object_type_id() {
        ObjectTypeId::GameObject => EntityKind::GameObject,
        ObjectTypeId::Unit => EntityKind::Unit,
        ObjectTypeId::Player => EntityKind::Player,
        _ => EntityKind::Unknown,
    };
    EntityState {
        id,
        entry,
        kind,
        name: names.and_then(|registry| registry.name_for(object)).map(str::to_owned),
        position,
        health,
        power,
        power_type,
        target,
        hostile: false,
        interactable,
    }
}

pub fn upsert_object(object: &Object, names: Option<&ObjectNameRegistry>, map_id: u32) -> ProtocolObservation {
    ProtocolObservation::EntityUpsert { entity: object_to_entity(object, names, map_id) }
}
