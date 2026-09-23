use std::{collections::BTreeMap, sync::Arc};

use anyhow::Result;
use tentacli::{
    client::prelude::{CtxMap, HandlerOutput, Packet, PacketOpcode, PacketType, Processor, Request},
    plugins::wow::wotlk::realm::object::{
        object_names, objects, ObjectProcessor,
        types::update_fields::{FieldValue, ItemField, PlayerField, UnitField},
    },
};
use tokio::sync::RwLock;
use wow_domain::{EntityId, WorldPosition};
use wow_state::ProtocolObservation;

use crate::objects::object_to_entity;

/// Stateful bridge around Tentacli's WotLK ObjectProcessor.
///
/// The proxy owns this adapter per configured WorldSession. Tentacli maintains
/// packet-derived object state; wow-bot then projects that state into typed
/// ProtocolObservation values. The Tentacli context itself never becomes the
/// bot's authoritative state.
pub struct ObjectObservationRuntime {
    processor: ObjectProcessor,
    context: Arc<RwLock<CtxMap>>,
    known: BTreeMap<EntityId, wow_state::entities::EntityState>,
    known_quests: BTreeMap<u32, (bool, Vec<u32>)>,
    known_inventory: BTreeMap<u32, u32>,
    known_inventory_instances: Vec<wow_state::inventory::InventoryItemInstance>,
    known_controlled_mover: Option<EntityId>,
    last_controlled_position: Option<WorldPosition>,
    last_controlled_flags: u32,
    last_money: Option<u64>,
    known_equipped_ranged_item: Option<u32>,
    equipment_observed: bool,
    last_class_id: Option<u8>,
    last_player_position: Option<WorldPosition>,
    map_id: u32,
    player_guid: Option<EntityId>,
}

impl ObjectObservationRuntime {
    pub fn new() -> Result<Self> {
        let mut processor = ObjectProcessor::default();
        let mut context = CtxMap::default();
        processor.init(&mut context)?;
        Ok(Self {
            processor,
            context: Arc::new(RwLock::new(context)),
            known: BTreeMap::new(),
            known_quests: BTreeMap::new(),
            known_inventory: BTreeMap::new(),
            known_inventory_instances: Vec::new(),
            known_controlled_mover: None,
            last_controlled_position: None,
            last_controlled_flags: 0,
            last_money: None,
            known_equipped_ranged_item: None,
            equipment_observed: false,
            last_class_id: None,
            last_player_position: None,
            map_id: 0,
            player_guid: None,
        })
    }

    pub fn set_world(&mut self, map_id: u32, player_guid: Option<EntityId>) {
        self.map_id = map_id;
        if player_guid.is_some() {
            self.player_guid = player_guid;
        }
    }

    pub fn set_player_guid(&mut self, player_guid: EntityId) {
        self.player_guid = Some(player_guid);
    }

    pub async fn observe(&mut self, opcode: u16, body: &[u8]) -> Result<Vec<ProtocolObservation>> {
        let mut packet = Packet::default();
        packet.set_type(PacketType::Incoming);
        packet.set_opcode(PacketOpcode::U16(opcode));
        packet.set_packet_size(body.len());
        packet.set_body(body.to_vec());

        if let Some(outputs) = self.processor.process(&mut packet, self.context.clone()).await? {
            self.apply_outputs(outputs).await;
        }

        let mut observations = Vec::new();
        let mut current = BTreeMap::new();
        {
            let guard = self.context.read().await;
            let mut inventory = BTreeMap::<u32, u32>::new();
            let mut inventory_instances = Vec::new();
            if let Some(map) = objects(&guard) {
                let names = object_names(&guard);
                let controlled_mover = self.player_guid.and_then(|player| {
                    map.values().find(|object| object.guid().0 == player.0).and_then(controlled_mover_of)
                });
                let controlled_object = controlled_mover.and_then(|mover| map.values().find(|object| object.guid().0 == mover.0));
                let controlled_position = controlled_object.and_then(|object| object_to_entity(object, names, self.map_id).position);
                let controlled_flags = controlled_object
                    .and_then(|object| object.movement.as_ref())
                    .and_then(|movement| movement.movement_info.as_ref())
                    .map(|info| info.movement_flags.bits())
                    .unwrap_or_default();
                if controlled_mover != self.known_controlled_mover || controlled_position != self.last_controlled_position || controlled_flags != self.last_controlled_flags {
                    observations.push(ProtocolObservation::ControlledMover { mover: controlled_mover, position: controlled_position, flags: controlled_flags });
                    self.known_controlled_mover = controlled_mover;
                    self.last_controlled_position = controlled_position;
                    self.last_controlled_flags = controlled_flags;
                }
                let backpack_slots: BTreeMap<u64, u8> = self.player_guid.and_then(|player| map.values().find(|object| object.guid().0 == player.0)).map(backpack_slots_of).unwrap_or_default();
                let equipped_ranged_guid = self.player_guid
                    .and_then(|player| map.values().find(|object| object.guid().0 == player.0))
                    .and_then(equipped_ranged_guid_of);
                let equipped_ranged_item = equipped_ranged_guid.and_then(|guid| map.values().find(|object| object.guid().0 == guid).and_then(|object| object.entry_id()));
                if !self.equipment_observed || equipped_ranged_item != self.known_equipped_ranged_item {
                    observations.push(ProtocolObservation::EquippedRangedItem { item: equipped_ranged_item });
                    self.known_equipped_ranged_item = equipped_ranged_item;
                    self.equipment_observed = true;
                }
                for object in map.values() {
                    let entity = object_to_entity(object, names, self.map_id);
                    if self.player_guid.is_some_and(|player| item_owned_by(object, player)) {
                        if let (Some(entry), Some(stack)) = (object.entry_id(), item_stack_count(object)) {
                            if entry != 0 && stack != 0 {
                                *inventory.entry(entry).or_default() = inventory.get(&entry).copied().unwrap_or_default().saturating_add(stack);
                                if let Some(&slot) = backpack_slots.get(&object.guid().0) {
                                    inventory_instances.push(wow_state::inventory::InventoryItemInstance { item: entry, guid: EntityId(object.guid().0), backpack_slot: slot, count: stack });
                                }
                            }
                        }
                    }
                    if self.player_guid == Some(entity.id) {
                        if let Some(class_id) = object.as_player().and_then(|player| player.class()).map(|class| class as u8) {
                            if self.last_class_id != Some(class_id) {
                                observations.push(ProtocolObservation::PlayerClass { class_id });
                                self.last_class_id = Some(class_id);
                            }
                        }
                        if let Some(position) = entity.position {
                            if self.last_player_position != Some(position) {
                                observations.push(ProtocolObservation::PlayerPosition {
                                    position,
                                    moving: false,
                                    flags: 0,
                                    client_time: 0,
                                });
                                self.last_player_position = Some(position);
                            }
                        }
                        let current_quests = quest_journal(object);
                        for (&quest, (complete, objectives)) in &current_quests {
                            if self.known_quests.get(&quest) != Some(&(*complete, objectives.clone())) {
                                observations.push(ProtocolObservation::QuestProgress {
                                    quest,
                                    objectives: objectives.clone(),
                                    complete: *complete,
                                });
                            }
                        }
                        for removed in self.known_quests.keys().filter(|quest| !current_quests.contains_key(quest)).copied() {
                            observations.push(ProtocolObservation::QuestRemoved { quest: removed });
                        }
                        self.known_quests = current_quests;
                        if let Some(money) = player_money(object) {
                            if self.last_money != Some(money) {
                                observations.push(ProtocolObservation::Money { copper: money });
                                self.last_money = Some(money);
                            }
                        }
                    }
                    if self.known.get(&entity.id) != Some(&entity) {
                        observations.push(ProtocolObservation::EntityUpsert { entity: entity.clone() });
                    }
                    current.insert(entity.id, entity);
                }
            }
            let mut inventory_entries: std::collections::BTreeSet<u32> = self.known_inventory.keys().copied().collect();
            inventory_entries.extend(inventory.keys().copied());
            for item in inventory_entries {
                let count = inventory.get(&item).copied().unwrap_or_default();
                if self.known_inventory.get(&item).copied().unwrap_or_default() != count {
                    observations.push(ProtocolObservation::InventoryCount { item, count });
                }
            }
            self.known_inventory = inventory;
            inventory_instances.sort_by_key(|item| (item.item, item.backpack_slot, item.guid.0));
            if inventory_instances != self.known_inventory_instances {
                observations.push(ProtocolObservation::InventoryInstances { items: inventory_instances.clone() });
                self.known_inventory_instances = inventory_instances;
            }
        }
        for removed in self.known.keys().filter(|id| !current.contains_key(id)).copied() {
            observations.push(ProtocolObservation::EntityRemoved { entity: removed });
        }
        self.known = current;
        Ok(observations)
    }

    async fn apply_outputs(&self, outputs: Vec<HandlerOutput>) {
        for output in outputs {
            if let HandlerOutput::Requests(requests) = output {
                for request in requests {
                    if let Request::SetContext(Some(callback)) = request {
                        let mut guard = self.context.write().await;
                        callback(&mut guard);
                    }
                }
            }
        }
    }
}

fn controlled_mover_of(object: &tentacli::plugins::wow::wotlk::realm::object::Object) -> Option<EntityId> {
    match object.unit_fields.get(&UnitField::Charm) {
        Some(FieldValue::Long(value)) if *value != 0 => Some(EntityId(*value)),
        _ => None,
    }
}

fn equipped_ranged_guid_of(object: &tentacli::plugins::wow::wotlk::realm::object::Object) -> Option<u64> {
    // PLAYER_FIELD_INV_SLOT_HEAD is an array of 23 GUIDs. Equipment slot 17 is ranged/relic in 3.3.5a.
    match object.player_fields.get(&PlayerField::InvSlot) {
        Some(FieldValue::LongArray(values)) => values.get(17).and_then(|value| *value).filter(|guid| *guid != 0),
        _ => None,
    }
}

fn backpack_slots_of(object: &tentacli::plugins::wow::wotlk::realm::object::Object) -> BTreeMap<u64, u8> {
    let mut slots = BTreeMap::new();
    let Some(FieldValue::LongArray(values)) = object.player_fields.get(&PlayerField::PackSlot) else { return slots; };
    for (index, guid) in values.iter().enumerate() {
        if let Some(guid) = *guid {
            if guid != 0 {
                if let Ok(slot) = u8::try_from(23usize + index) { slots.insert(guid, slot); }
            }
        }
    }
    slots
}

fn item_owned_by(object: &tentacli::plugins::wow::wotlk::realm::object::Object, player: EntityId) -> bool {
    let Some(FieldValue::Long(owner)) = object.item_fields.get(&ItemField::Owner) else {
        return false;
    };
    *owner == player.0
}

fn item_stack_count(object: &tentacli::plugins::wow::wotlk::realm::object::Object) -> Option<u32> {
    match object.item_fields.get(&ItemField::StackCount) {
        Some(FieldValue::Integer(value)) if *value > 0 => u32::try_from(*value).ok(),
        _ => None,
    }
}

fn player_money(object: &tentacli::plugins::wow::wotlk::realm::object::Object) -> Option<u64> {
    match object.player_fields.get(&PlayerField::Coinage) {
        Some(FieldValue::Integer(value)) if *value >= 0 => Some(*value as u64),
        _ => None,
    }
}

fn quest_journal(object: &tentacli::plugins::wow::wotlk::realm::object::Object) -> BTreeMap<u32, (bool, Vec<u32>)> {
    let mut quests = BTreeMap::new();
    let Some(FieldValue::CustomArray(rows)) = object.player_fields.get(&PlayerField::QuestLog) else {
        return quests;
    };
    for row in rows {
        let quest = row.first().and_then(Option::as_ref).and_then(|value| match value {
            FieldValue::Integer(value) if *value > 0 => u32::try_from(*value).ok(),
            _ => None,
        });
        let state = row.get(1).and_then(Option::as_ref).and_then(|value| match value {
            FieldValue::Integer(value) => u32::try_from(*value).ok(),
            _ => None,
        }).unwrap_or_default();
        let pair = |index: usize| -> (u32, u32) {
            row.get(index)
                .and_then(Option::as_ref)
                .and_then(|value| match value {
                    FieldValue::TwoShorts((a, b)) => Some((u16::from_ne_bytes(a.to_ne_bytes()) as u32, u16::from_ne_bytes(b.to_ne_bytes()) as u32)),
                    _ => None,
                })
                .unwrap_or_default()
        };
        let (o1, o2) = pair(2);
        let (o3, o4) = pair(3);
        if let Some(quest) = quest {
            quests.insert(quest, (state & 0x0001 != 0, vec![o1, o2, o3, o4]));
        }
    }
    quests
}

pub fn login_verify_world(body: &[u8], character_guid: u64) -> Option<ProtocolObservation> {
    let map = u32::from_le_bytes(body.get(0..4)?.try_into().ok()?);
    let x = f32::from_le_bytes(body.get(4..8)?.try_into().ok()?);
    let y = f32::from_le_bytes(body.get(8..12)?.try_into().ok()?);
    let z = f32::from_le_bytes(body.get(12..16)?.try_into().ok()?);
    let orientation = f32::from_le_bytes(body.get(16..20)?.try_into().ok()?);
    let position = WorldPosition {
        map,
        point: wow_domain::Vec3::new(x, y, z),
        orientation,
    };
    position.point.is_finite().then_some(ProtocolObservation::EnteredWorld {
        character_guid,
        position: Some(position),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_azerothcore_login_verify_world_layout() {
        let mut body = Vec::new();
        body.extend_from_slice(&1_u32.to_le_bytes());
        body.extend_from_slice(&1.0_f32.to_le_bytes());
        body.extend_from_slice(&2.0_f32.to_le_bytes());
        body.extend_from_slice(&3.0_f32.to_le_bytes());
        body.extend_from_slice(&4.0_f32.to_le_bytes());
        let observation = login_verify_world(&body, 77).expect("valid verify-world packet");
        assert!(matches!(observation, ProtocolObservation::EnteredWorld { character_guid: 77, .. }));
    }
}
