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
    // WoW update fields omit values that remain at their protocol default. The
    // object processor also keeps partial arrays, so unset slots mean zero.
    let (power_cost_modifiers, power_cost_multipliers) =
        if object.object_type_id() == ObjectTypeId::Player {
            (
                Some(unit_integer_array_or_default(
                    object,
                    UnitField::PowerCostModifier,
                )),
                Some(unit_float_array_or_default(
                    object,
                    UnitField::PowerCostMultiplier,
                )),
            )
        } else {
            (
                unit_integer_array(object, UnitField::PowerCostModifier),
                unit_float_array(object, UnitField::PowerCostMultiplier),
            )
        };
    let base_attack_time_ms = unit_unsigned_array::<2>(object, UnitField::BaseAttackTime);
    let shapeshift_form = match object.unit_fields.get(&UnitField::Bytes2) {
        Some(FieldValue::Bytes(value)) => Some((value >> 24) as u8),
        None if object.object_type_id() == ObjectTypeId::Player => Some(0),
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
    complete_array::<7, _, _>(values, Some)
}

fn unit_float_array(object: &Object, field: UnitField) -> Option<[f32; 7]> {
    let Some(FieldValue::FloatArray(values)) = object.unit_fields.get(&field) else {
        return None;
    };
    complete_array::<7, _, _>(values, Some)
}

fn unit_integer_array_or_default(object: &Object, field: UnitField) -> [i32; 7] {
    let Some(FieldValue::IntegerArray(values)) = object.unit_fields.get(&field) else {
        return [0; 7];
    };
    std::array::from_fn(|index| values.get(index).copied().flatten().unwrap_or_default())
}

fn unit_float_array_or_default(object: &Object, field: UnitField) -> [f32; 7] {
    let Some(FieldValue::FloatArray(values)) = object.unit_fields.get(&field) else {
        return [0.0; 7];
    };
    std::array::from_fn(|index| values.get(index).copied().flatten().unwrap_or_default())
}

fn unit_unsigned_array<const N: usize>(object: &Object, field: UnitField) -> Option<[u32; N]> {
    let Some(FieldValue::IntegerArray(values)) = object.unit_fields.get(&field) else {
        return None;
    };
    complete_array::<N, _, _>(values, |value| u32::try_from(value).ok())
}

fn complete_array<const N: usize, T: Copy, U>(
    values: &[Option<T>],
    convert: impl Fn(T) -> Option<U>,
) -> Option<[U; N]> {
    values
        .iter()
        .copied()
        .map(|value| value.and_then(&convert))
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tentacli::plugins::wow::wotlk::realm::object::{
        Object, ObjectTypeId, PackedGuid,
        types::{
            update_data::ObjectTypeMask,
            update_fields::{FieldValue, UnitField},
        },
    };

    use super::{complete_array, object_to_entity};

    fn object(object_type_id: ObjectTypeId, bytes2: Option<u8>) -> Object {
        let mut unit_fields = BTreeMap::new();
        if let Some(form) = bytes2 {
            unit_fields.insert(UnitField::Bytes2, FieldValue::Bytes(u32::from(form) << 24));
        }
        Object {
            guid: PackedGuid(1),
            object_type_id,
            object_type_mask: ObjectTypeMask::default(),
            movement: None,
            object_fields: BTreeMap::new(),
            unit_fields,
            player_fields: BTreeMap::new(),
            item_fields: BTreeMap::new(),
            container_fields: BTreeMap::new(),
            game_object_fields: BTreeMap::new(),
            dynamic_object_fields: BTreeMap::new(),
            corpse_fields: BTreeMap::new(),
        }
    }

    #[test]
    fn complete_array_checks_presence_length_and_conversion() {
        assert_eq!(
            complete_array::<3, _, _>(&[Some(1), Some(2), Some(3)], Some),
            Some([1, 2, 3])
        );
        assert!(complete_array::<3, _, _>(&[Some(1), None, Some(3)], Some).is_none());
        assert!(complete_array::<2, _, _>(&[Some(1), Some(2), Some(3)], Some).is_none());
        assert_eq!(
            complete_array::<2, _, _>(&[Some(1_i32), Some(-1)], |value| {
                u32::try_from(value).ok()
            }),
            None
        );
    }

    #[test]
    fn player_with_omitted_bytes2_uses_unshifted_default_form() {
        let entity = object_to_entity(&object(ObjectTypeId::Player, None), None, 0);

        assert_eq!(entity.shapeshift_form, Some(0));
    }

    #[test]
    fn player_with_observed_travel_form_keeps_that_form() {
        // Form 3 is the travel form used by the test server. Keep it for any
        // player class when the server sends it in Bytes2.
        let entity = object_to_entity(&object(ObjectTypeId::Player, Some(3)), None, 0);

        assert_eq!(entity.shapeshift_form, Some(3));
    }

    #[test]
    fn unit_with_omitted_bytes2_remains_unknown() {
        let entity = object_to_entity(&object(ObjectTypeId::Unit, None), None, 0);

        assert_eq!(entity.shapeshift_form, None);
    }
}
