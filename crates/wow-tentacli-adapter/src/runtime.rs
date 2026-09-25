use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::Result;
use tentacli::{
    client::prelude::{
        CtxMap, HandlerOutput, Packet, PacketOpcode, PacketType, Processor, Request,
    },
    plugins::wow::wotlk::realm::object::{
        Object, ObjectMap, ObjectProcessor, object_names, objects,
        types::update_fields::{FieldValue, ItemField, PlayerField, UnitField},
    },
};
use tokio::sync::RwLock;
use wow_domain::binary::{array_at, u16_le, u32_le};
use wow_domain::{EntityId, WorldPosition};
use wow_state::{ProtocolObservation, capabilities::TalentRank};

use crate::objects::object_to_entity;

const SMSG_TALENTS_INFO: u16 = 0x04c0;
const SMSG_PET_SPELLS: u16 = 0x0179;

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
    known_equipped_items: Option<BTreeMap<u8, u32>>,
    equipment_slots_observed: bool,
    profession_skill_info_observed: bool,
    known_profession_skills: Option<(BTreeMap<u32, (u16, u16)>, BTreeMap<usize, u32>)>,
    pet_control_observed: bool,
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
            known_equipped_items: None,
            equipment_slots_observed: false,
            profession_skill_info_observed: false,
            known_profession_skills: None,
            pet_control_observed: false,
            last_class_id: None,
            last_player_position: None,
            map_id: 0,
            player_guid: None,
        })
    }

    pub fn set_world(&mut self, map_id: u32, player_guid: Option<EntityId>) {
        if player_guid.is_some() {
            self.profession_skill_info_observed = false;
            self.known_profession_skills = None;
        }
        self.map_id = map_id;
        if player_guid.is_some() {
            self.player_guid = player_guid;
        }
    }

    pub fn set_player_guid(&mut self, player_guid: EntityId) {
        self.player_guid = Some(player_guid);
    }

    pub async fn observe(&mut self, opcode: u16, body: &[u8]) -> Result<Vec<ProtocolObservation>> {
        let mut observations = Vec::new();
        if opcode == SMSG_PET_SPELLS && body.len() >= 8 {
            let raw_pet = u64::from_le_bytes(body[..8].try_into().unwrap_or_default());
            observations.push(ProtocolObservation::PetControl {
                pet: (raw_pet != 0).then_some(EntityId(raw_pet)),
            });
            self.pet_control_observed = true;
        } else if opcode == SMSG_TALENTS_INFO
            && body.first().copied() == Some(0)
            && !self.pet_control_observed
        {
            // AzerothCore sends the player talent packet after pet initialization.
            // If no pet-spell packet arrived first, this confirms no active pet.
            observations.push(ProtocolObservation::PetControl { pet: None });
            self.pet_control_observed = true;
        }
        let mut packet = Packet::default();
        packet.set_type(PacketType::Incoming);
        packet.set_opcode(PacketOpcode::U16(opcode));
        packet.set_packet_size(body.len());
        packet.set_body(body.to_vec());

        if let Some(outputs) = self
            .processor
            .process(&mut packet, self.context.clone())
            .await?
        {
            self.apply_outputs(outputs).await;
        }

        if opcode == SMSG_TALENTS_INFO
            && let Some((group_count, active_group, talents)) = parse_player_talents_info(body)
        {
            observations.push(ProtocolObservation::PlayerTalents {
                group_count,
                active_group,
                talents,
            });
        }
        let mut current = BTreeMap::new();
        {
            let guard = self.context.read().await;
            let mut inventory = BTreeMap::<u32, u32>::new();
            let mut inventory_instances = Vec::new();
            if let Some(map) = objects(&guard) {
                let names = object_names(&guard);
                let controlled_mover = self.player_guid.and_then(|player| {
                    map.values()
                        .find(|object| object.guid().0 == player.0)
                        .and_then(controlled_mover_of)
                });
                let controlled_object = controlled_mover
                    .and_then(|mover| map.values().find(|object| object.guid().0 == mover.0));
                let controlled_position = controlled_object
                    .and_then(|object| object_to_entity(object, names, self.map_id).position);
                let controlled_flags = controlled_object
                    .and_then(|object| object.movement.as_ref())
                    .and_then(|movement| movement.movement_info.as_ref())
                    .map(|info| info.movement_flags.bits())
                    .unwrap_or_default();
                if controlled_mover != self.known_controlled_mover
                    || controlled_position != self.last_controlled_position
                    || controlled_flags != self.last_controlled_flags
                {
                    observations.push(ProtocolObservation::ControlledMover {
                        mover: controlled_mover,
                        position: controlled_position,
                        flags: controlled_flags,
                    });
                    self.known_controlled_mover = controlled_mover;
                    self.last_controlled_position = controlled_position;
                    self.last_controlled_flags = controlled_flags;
                }
                let backpack_slots: BTreeMap<u64, u8> = self
                    .player_guid
                    .and_then(|player| map.values().find(|object| object.guid().0 == player.0))
                    .map(backpack_slots_of)
                    .unwrap_or_default();
                let equipped_ranged_guid = self
                    .player_guid
                    .and_then(|player| map.values().find(|object| object.guid().0 == player.0))
                    .and_then(equipped_ranged_guid_of);
                let equipped_ranged_item = equipped_ranged_guid.and_then(|guid| {
                    map.values()
                        .find(|object| object.guid().0 == guid)
                        .and_then(|object| object.entry_id())
                });
                if !self.equipment_observed
                    || equipped_ranged_item != self.known_equipped_ranged_item
                {
                    observations.push(ProtocolObservation::EquippedRangedItem {
                        item: equipped_ranged_item,
                    });
                    self.known_equipped_ranged_item = equipped_ranged_item;
                    self.equipment_observed = true;
                }
                for object in map.values() {
                    let entity = object_to_entity(object, names, self.map_id);
                    if self.player_guid == Some(entity.id)
                        && self.known.get(&entity.id).is_none_or(|previous| {
                            previous.power_type != entity.power_type
                                || previous.power != entity.power
                                || previous.shapeshift_form != entity.shapeshift_form
                        })
                    {
                        tracing::info!(
                            player=?entity.id,
                            power_type=?entity.power_type,
                            power=?entity.power,
                            shapeshift_form=?entity.shapeshift_form,
                            raw_bytes0=?object.unit_fields.get(&UnitField::Bytes0),
                            raw_bytes2=?object.unit_fields.get(&UnitField::Bytes2),
                            raw_powers=?object.unit_fields.get(&UnitField::Powers),
                            raw_max_powers=?object.unit_fields.get(&UnitField::MaxPowers),
                            "player combat state observation updated"
                        );
                    }
                    if self
                        .player_guid
                        .is_some_and(|player| item_owned_by(object, player))
                    {
                        if let (Some(entry), Some(stack)) =
                            (object.entry_id(), item_stack_count(object))
                        {
                            if entry != 0 && stack != 0 {
                                *inventory.entry(entry).or_default() = inventory
                                    .get(&entry)
                                    .copied()
                                    .unwrap_or_default()
                                    .saturating_add(stack);
                                if let Some(&slot) = backpack_slots.get(&object.guid().0) {
                                    inventory_instances.push(
                                        wow_state::inventory::InventoryItemInstance {
                                            item: entry,
                                            guid: EntityId(object.guid().0),
                                            backpack_slot: slot,
                                            count: stack,
                                        },
                                    );
                                }
                            }
                        }
                    }
                    if self.player_guid == Some(entity.id) {
                        if let Some((skills, slots)) = object
                            .player_fields
                            .get(&PlayerField::SkillInfo)
                            .and_then(profession_skills_from_field)
                        {
                            if !self.profession_skill_info_observed
                                || self.known_profession_skills.as_ref()
                                    != Some(&(skills.clone(), slots.clone()))
                            {
                                observations.push(ProtocolObservation::ProfessionSnapshot {
                                    skills: skills.clone(),
                                    slots: slots.clone(),
                                });
                                self.known_profession_skills = Some((skills, slots));
                                self.profession_skill_info_observed = true;
                            }
                        }
                        let equipped_items = equipped_items_of(object, map);
                        if !self.equipment_slots_observed
                            || equipped_items != self.known_equipped_items
                        {
                            observations.push(ProtocolObservation::EquippedItems {
                                items: equipped_items.clone(),
                            });
                            self.known_equipped_items = equipped_items;
                            self.equipment_slots_observed = true;
                        }
                        if let Some(class_id) = object
                            .as_player()
                            .and_then(|player| player.class())
                            .map(|class| class as u8)
                        {
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
                            if self.known_quests.get(&quest)
                                != Some(&(*complete, objectives.clone()))
                            {
                                observations.push(ProtocolObservation::QuestProgress {
                                    quest,
                                    objectives: objectives.clone(),
                                    complete: *complete,
                                });
                            }
                        }
                        for removed in self
                            .known_quests
                            .keys()
                            .filter(|quest| !current_quests.contains_key(quest))
                            .copied()
                        {
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
                        observations.push(ProtocolObservation::EntityUpsert {
                            entity: entity.clone(),
                        });
                    }
                    current.insert(entity.id, entity);
                }
            }
            let mut inventory_entries: std::collections::BTreeSet<u32> =
                self.known_inventory.keys().copied().collect();
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
                observations.push(ProtocolObservation::InventoryInstances {
                    items: inventory_instances.clone(),
                });
                self.known_inventory_instances = inventory_instances;
            }
        }
        for removed in self
            .known
            .keys()
            .filter(|id| !current.contains_key(id))
            .copied()
        {
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

fn controlled_mover_of(
    object: &tentacli::plugins::wow::wotlk::realm::object::Object,
) -> Option<EntityId> {
    match object.unit_fields.get(&UnitField::Charm) {
        Some(FieldValue::Long(value)) if *value != 0 => Some(EntityId(*value)),
        _ => None,
    }
}

/// Project the canonical WotLK SkillInfo update field into skill-line ranks.
/// Each skill uses three TwoShorts entries: skill/step, current/maximum, bonuses.
fn profession_skills_from_field(
    field: &FieldValue,
) -> Option<(BTreeMap<u32, (u16, u16)>, BTreeMap<usize, u32>)> {
    let FieldValue::TwoShortsArray(entries) = field else {
        return None;
    };
    let mut skills = BTreeMap::new();
    let mut slots = BTreeMap::new();
    for (slot, chunk) in entries.chunks(3).enumerate() {
        let Some((skill, _step)) = chunk.first().and_then(|value| *value) else {
            continue;
        };
        let skill = skill as u16;
        if skill == 0 {
            continue;
        }
        slots.insert(slot, u32::from(skill));
        let Some((current, maximum)) = chunk.get(1).and_then(|value| *value) else {
            continue;
        };
        skills.insert(skill as u32, (current as u16, maximum as u16));
    }
    Some((skills, slots))
}

fn equipped_ranged_guid_of(object: &Object) -> Option<u64> {
    // PLAYER_FIELD_INV_SLOT_HEAD is an array of 23 GUIDs. Equipment slot 17 is ranged/relic in 3.3.5a.
    inventory_slot_guids(object)?
        .get(17)
        .and_then(|value| *value)
        .filter(|guid| *guid != 0)
}

fn equipped_items_of(player: &Object, objects: &ObjectMap) -> Option<BTreeMap<u8, u32>> {
    let slots = inventory_slot_guids(player)?;
    let mut equipped = BTreeMap::new();
    for (slot, guid) in slots.iter().take(19).enumerate() {
        let Some(guid) = guid.filter(|guid| *guid != 0) else {
            continue;
        };
        let item = objects
            .values()
            .find(|object| object.guid().0 == guid)
            .and_then(|object| object.entry_id())?;
        equipped.insert(u8::try_from(slot).ok()?, item);
    }
    Some(equipped)
}

fn inventory_slot_guids(object: &Object) -> Option<&[Option<u64>]> {
    match object.player_fields.get(&PlayerField::InvSlot) {
        Some(FieldValue::LongArray(values)) => Some(values),
        _ => None,
    }
}

fn backpack_slots_of(
    object: &tentacli::plugins::wow::wotlk::realm::object::Object,
) -> BTreeMap<u64, u8> {
    let mut slots = BTreeMap::new();
    let Some(FieldValue::LongArray(values)) = object.player_fields.get(&PlayerField::PackSlot)
    else {
        return slots;
    };
    for (index, guid) in values.iter().enumerate() {
        if let Some(guid) = *guid {
            if guid != 0 {
                if let Ok(slot) = u8::try_from(23usize + index) {
                    slots.insert(guid, slot);
                }
            }
        }
    }
    slots
}

fn item_owned_by(
    object: &tentacli::plugins::wow::wotlk::realm::object::Object,
    player: EntityId,
) -> bool {
    let Some(FieldValue::Long(owner)) = object.item_fields.get(&ItemField::Owner) else {
        return false;
    };
    *owner == player.0
}

fn item_stack_count(object: &tentacli::plugins::wow::wotlk::realm::object::Object) -> Option<u32> {
    object
        .item_fields
        .get(&ItemField::StackCount)
        .and_then(field_u32)
        .filter(|count| *count > 0)
}

fn field_u32(value: &FieldValue) -> Option<u32> {
    match value {
        FieldValue::Integer(value) => u32::try_from(*value).ok(),
        _ => None,
    }
}

fn player_money(object: &tentacli::plugins::wow::wotlk::realm::object::Object) -> Option<u64> {
    match object.player_fields.get(&PlayerField::Coinage) {
        Some(FieldValue::Integer(value)) if *value >= 0 => Some(*value as u64),
        _ => None,
    }
}

fn quest_journal(
    object: &tentacli::plugins::wow::wotlk::realm::object::Object,
) -> BTreeMap<u32, (bool, Vec<u32>)> {
    let mut quests = BTreeMap::new();
    let Some(FieldValue::CustomArray(rows)) = object.player_fields.get(&PlayerField::QuestLog)
    else {
        return quests;
    };
    for row in rows {
        let quest = row
            .first()
            .and_then(Option::as_ref)
            .and_then(field_u32)
            .filter(|quest| *quest > 0);
        let state = row
            .get(1)
            .and_then(Option::as_ref)
            .and_then(field_u32)
            .unwrap_or_default();
        let pair = |index: usize| -> (u32, u32) {
            row.get(index)
                .and_then(Option::as_ref)
                .and_then(|value| match value {
                    FieldValue::TwoShorts((a, b)) => Some((
                        u16::from_ne_bytes(a.to_ne_bytes()) as u32,
                        u16::from_ne_bytes(b.to_ne_bytes()) as u32,
                    )),
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
    position
        .point
        .is_finite()
        .then_some(ProtocolObservation::EnteredWorld {
            character_guid,
            position: Some(position),
        })
}

/// Parse AzerothCore's player SMSG_TALENTS_INFO payload. Pet packets use marker 1
/// and are ignored. A malformed player packet emits unknown talent data so stale
/// specialization state cannot remain active.
fn parse_player_talents_info(body: &[u8]) -> Option<(Option<u8>, Option<u8>, Vec<TalentRank>)> {
    if body.first().copied()? != 0 {
        return None;
    }
    let invalid = || Some((None, None, Vec::new()));
    let mut reader = TalentPacketReader::new(&body[1..]);
    let Some(_unspent_points) = reader.u32() else {
        return invalid();
    };
    let Some(group_count) = reader.u8() else {
        return invalid();
    };
    let Some(active_group) = reader.u8() else {
        return invalid();
    };
    if !(1..=2).contains(&group_count) || active_group >= group_count {
        return invalid();
    }

    let mut active_talents = None;
    for group in 0..group_count {
        let Some(count) = reader.u8().map(usize::from) else {
            return invalid();
        };
        if count > 150 {
            return invalid();
        }
        let mut talents = Vec::with_capacity(count);
        let mut talent_ids = BTreeSet::new();
        for _ in 0..count {
            let (Some(talent_id), Some(rank)) = (reader.u32(), reader.u8()) else {
                return invalid();
            };
            if talent_id == 0 || rank >= 5 || !talent_ids.insert(talent_id) {
                return invalid();
            }
            talents.push(TalentRank { talent_id, rank });
        }
        let Some(glyph_count) = reader.u8().map(usize::from) else {
            return invalid();
        };
        if glyph_count > 6 {
            return invalid();
        }
        for _ in 0..glyph_count {
            if reader.u16().is_none() {
                return invalid();
            }
        }
        if group == active_group {
            active_talents = Some(talents);
        }
    }
    if !reader.is_empty() {
        return invalid();
    }
    Some((
        Some(group_count),
        Some(active_group),
        active_talents.unwrap_or_default(),
    ))
}

struct TalentPacketReader<'a> {
    body: &'a [u8],
    offset: usize,
}

impl<'a> TalentPacketReader<'a> {
    fn new(body: &'a [u8]) -> Self {
        Self { body, offset: 0 }
    }

    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let bytes = array_at(self.body, self.offset)?;
        let end = self.offset.checked_add(N)?;
        self.offset = end;
        Some(bytes)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take::<1>()?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        u16_le(&self.take::<2>()?)
    }

    fn u32(&mut self) -> Option<u32> {
        u32_le(&self.take::<4>()?)
    }

    fn is_empty(&self) -> bool {
        self.offset == self.body.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pet_control_uses_pet_bar_and_initial_talents_as_authority() {
        let mut runtime = ObjectObservationRuntime::new().expect("object observer");
        let mut pet_packet = vec![0; 58];
        pet_packet[..8].copy_from_slice(&55_u64.to_le_bytes());
        let observations = runtime
            .observe(SMSG_PET_SPELLS, &pet_packet)
            .await
            .expect("pet packet");
        assert!(matches!(
            observations.first(),
            Some(ProtocolObservation::PetControl {
                pet: Some(EntityId(55))
            })
        ));

        let talents = player_talents_packet(0, &[talent_group(&[], &[])]);
        let observations = runtime
            .observe(SMSG_TALENTS_INFO, &talents)
            .await
            .expect("talents packet");
        assert!(
            !observations
                .iter()
                .any(|observation| matches!(observation, ProtocolObservation::PetControl { .. }))
        );

        let mut no_pet_runtime = ObjectObservationRuntime::new().expect("object observer");
        let observations = no_pet_runtime
            .observe(SMSG_TALENTS_INFO, &talents)
            .await
            .expect("talents packet");
        assert!(matches!(
            observations.first(),
            Some(ProtocolObservation::PetControl { pet: None })
        ));
    }

    #[test]
    fn skill_info_projects_rank_and_maximum_by_skill_line() {
        let mut entries = vec![None; 384];
        entries[0] = Some((164, 0));
        entries[1] = Some((75, 150));
        entries[3] = Some((185, 0));
        entries[4] = Some((40, 75));
        let (skills, slots) = profession_skills_from_field(&FieldValue::TwoShortsArray(entries))
            .expect("canonical skill info");
        assert_eq!(skills.get(&164), Some(&(75, 150)));
        assert_eq!(skills.get(&185), Some(&(40, 75)));
        assert_eq!(slots.get(&0), Some(&164));
        assert_eq!(slots.get(&1), Some(&185));
    }

    fn talent_group(talents: &[(u32, u8)], glyphs: &[u16]) -> Vec<u8> {
        let mut body = vec![talents.len() as u8];
        for (talent_id, rank) in talents {
            body.extend_from_slice(&talent_id.to_le_bytes());
            body.push(*rank);
        }
        body.push(glyphs.len() as u8);
        for glyph in glyphs {
            body.extend_from_slice(&glyph.to_le_bytes());
        }
        body
    }

    fn player_talents_packet(active: u8, groups: &[Vec<u8>]) -> Vec<u8> {
        let mut body = vec![0];
        body.extend_from_slice(&0u32.to_le_bytes());
        body.push(groups.len() as u8);
        body.push(active);
        for group in groups {
            body.extend_from_slice(group);
        }
        body
    }

    #[test]
    fn parses_azerothcore_login_verify_world_layout() {
        let mut body = Vec::new();
        body.extend_from_slice(&1_u32.to_le_bytes());
        body.extend_from_slice(&1.0_f32.to_le_bytes());
        body.extend_from_slice(&2.0_f32.to_le_bytes());
        body.extend_from_slice(&3.0_f32.to_le_bytes());
        body.extend_from_slice(&4.0_f32.to_le_bytes());
        let observation = login_verify_world(&body, 77).expect("valid verify-world packet");
        assert!(matches!(
            observation,
            ProtocolObservation::EnteredWorld {
                character_guid: 77,
                ..
            }
        ));
    }

    #[test]
    fn field_u32_shares_checked_integer_conversion() {
        assert_eq!(field_u32(&FieldValue::Integer(12)), Some(12));
        assert_eq!(field_u32(&FieldValue::Integer(-1)), None);
        assert_eq!(field_u32(&FieldValue::Bytes(12)), None);
    }

    #[test]
    fn parses_active_talent_group_from_single_and_dual_spec_packets() {
        let single = player_talents_packet(0, &[talent_group(&[(74, 2)], &[123, 456])]);
        assert_eq!(
            parse_player_talents_info(&single),
            Some((
                Some(1),
                Some(0),
                vec![TalentRank {
                    talent_id: 74,
                    rank: 2,
                }]
            ))
        );

        let dual = player_talents_packet(
            1,
            &[
                talent_group(&[(74, 1)], &[]),
                talent_group(&[(27, 3), (26, 0)], &[7]),
            ],
        );
        let active_first = player_talents_packet(
            0,
            &[
                talent_group(&[(74, 1)], &[]),
                talent_group(&[(27, 3), (26, 0)], &[7]),
            ],
        );
        assert_eq!(
            parse_player_talents_info(&active_first),
            Some((
                Some(2),
                Some(0),
                vec![TalentRank {
                    talent_id: 74,
                    rank: 1,
                }]
            ))
        );
        assert_eq!(
            parse_player_talents_info(&dual),
            Some((
                Some(2),
                Some(1),
                vec![
                    TalentRank {
                        talent_id: 27,
                        rank: 3,
                    },
                    TalentRank {
                        talent_id: 26,
                        rank: 0,
                    }
                ]
            ))
        );
    }

    #[test]
    fn truncated_or_invalid_player_talents_are_reported_as_unknown() {
        assert_eq!(
            parse_player_talents_info(&[0]),
            Some((None, None, Vec::new()))
        );
        assert_eq!(
            parse_player_talents_info(&player_talents_packet(0, &[vec![1, 74, 0]])),
            Some((None, None, Vec::new()))
        );
        let mut invalid_active = player_talents_packet(2, &[talent_group(&[], &[])]);
        assert_eq!(
            parse_player_talents_info(&invalid_active),
            Some((None, None, Vec::new()))
        );
        invalid_active = player_talents_packet(0, &[talent_group(&[(0, 0)], &[])]);
        assert_eq!(
            parse_player_talents_info(&invalid_active),
            Some((None, None, Vec::new()))
        );
        assert_eq!(parse_player_talents_info(&[1, 0, 0, 0, 0]), None);
    }
}
