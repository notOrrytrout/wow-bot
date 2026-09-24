use tentacli::plugins::wow::wotlk::realm::object::{
    Object, ObjectNameRegistry, ObjectTypeId,
    types::update_fields::{FieldValue, UnitField},
};
use wow_domain::{EntityId, Vec3, WorldPosition};
use wow_state::{
    ProtocolObservation,
    entities::{EntityKind, EntityState},
};

pub fn object_to_entity(
    object: &Object,
    names: Option<&ObjectNameRegistry>,
    map_id: u32,
) -> EntityState {
    let id = EntityId(object.guid().0);
    let entry = object.entry_id().unwrap_or_default();
    let position = object.position().map(|point| WorldPosition {
        map: map_id,
        point: Vec3::new(point.x, point.y, point.z),
        orientation: object.facing().unwrap_or_default(),
    });
    let health = object
        .as_unit()
        .and_then(|unit| unit.health().zip(unit.max_health()));
    let power = object
        .as_unit()
        .and_then(|unit| unit.power().zip(unit.max_power()));
    let power_type = object
        .as_unit()
        .and_then(|unit| unit.power_type())
        .map(u8::from);
    let level = object.as_unit().and_then(|unit| unit.level());
    let base_health =
        unit_integer(object, UnitField::BaseHealth).and_then(|value| u32::try_from(value).ok());
    let base_mana =
        unit_integer(object, UnitField::BaseMana).and_then(|value| u32::try_from(value).ok());
    let power_cost_modifiers = unit_integer_array(object, UnitField::PowerCostModifier);
    let power_cost_multipliers = unit_float_array(object, UnitField::PowerCostMultiplier);
    let base_attack_time_ms = unit_unsigned_array::<2>(object, UnitField::BaseAttackTime);
    let shapeshift_form = match object.unit_fields.get(&UnitField::Bytes2) {
        Some(FieldValue::Bytes(value)) => Some((value >> 24) as u8),
        _ => None,
    };
    let aura_state =
        unit_integer(object, UnitField::AuraState).and_then(|value| u32::try_from(value).ok());
    let target = match object.unit_fields.get(&UnitField::Target) {
        Some(FieldValue::Long(value)) if *value != 0 => Some(EntityId(*value)),
        _ => None,
    };
    let interactable = match object.object_type_id() {
        ObjectTypeId::GameObject => true,
        ObjectTypeId::Unit | ObjectTypeId::Player => {
            object
                .as_unit()
                .and_then(|unit| unit.npc_flags())
                .unwrap_or_default()
                != 0
        }
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
        name: names
            .and_then(|registry| registry.name_for(object))
            .map(str::to_owned),
        position,
        health,
        power,
        power_type,
        level,
        base_health,
        base_mana,
        power_cost_modifiers,
        power_cost_multipliers,
        base_attack_time_ms,
        shapeshift_form,
        aura_state,
        target,
        hostile: false,
        interactable,
    }
}

fn unit_integer(object: &Object, field: UnitField) -> Option<i32> {
    match object.unit_fields.get(&field) {
        Some(FieldValue::Integer(value)) => Some(*value),
        _ => None,
    }
}

fn unit_integer_array(object: &Object, field: UnitField) -> Option<[i32; 7]> {
    let Some(FieldValue::IntegerArray(values)) = object.unit_fields.get(&field) else {
        return None;
    };
    values
        .iter()
        .copied()
        .collect::<Option<Vec<_>>>()?
        .try_into()
        .ok()
}

fn unit_float_array(object: &Object, field: UnitField) -> Option<[f32; 7]> {
    let Some(FieldValue::FloatArray(values)) = object.unit_fields.get(&field) else {
        return None;
    };
    values
        .iter()
        .copied()
        .collect::<Option<Vec<_>>>()?
        .try_into()
        .ok()
}

fn unit_unsigned_array<const N: usize>(object: &Object, field: UnitField) -> Option<[u32; N]> {
    let Some(FieldValue::IntegerArray(values)) = object.unit_fields.get(&field) else {
        return None;
    };
    values
        .iter()
        .copied()
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .map(|value| u32::try_from(value).ok())
        .collect::<Option<Vec<_>>>()?
        .try_into()
        .ok()
}

pub fn upsert_object(
    object: &Object,
    names: Option<&ObjectNameRegistry>,
    map_id: u32,
) -> ProtocolObservation {
    ProtocolObservation::EntityUpsert {
        entity: object_to_entity(object, names, map_id),
    }
}
