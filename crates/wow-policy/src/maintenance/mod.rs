use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::OnceLock,
    time::{Duration, Instant},
};
use wow_domain::{EntityId, WorldPosition, time::Millis};
use wow_state::{Snapshot, group::GroupLifecycle};

#[derive(Clone, Debug, Deserialize)]
struct Catalog {
    policies: Vec<BuffFamilyPolicy>,
}
#[derive(Clone, Debug, Deserialize)]
struct GlyphCatalog {
    glyphs: Vec<GlyphDefinition>,
}
#[derive(Clone, Copy, Debug, Deserialize)]
struct GlyphDefinition {
    property_id: u16,
    spell_id: u32,
}
#[derive(Clone, Debug, Deserialize)]
pub struct BuffFamilyPolicy {
    pub class_id: u8,
    pub family: String,
    pub party: bool,
    pub spells: Vec<RankedSpell>,
}
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct RankedSpell {
    pub spell: u32,
    pub rank: u32,
    pub level: u32,
    pub strength: u32,
}

static CATALOG: OnceLock<Catalog> = OnceLock::new();
static GLYPHS: OnceLock<BTreeMap<u16, u32>> = OnceLock::new();
pub const REPAIR_DURABILITY_THRESHOLD_PERCENT: u8 = 25;
const ETERNAL_WATER_GLYPH_SPELL: u32 = 70_937;
const MAGE_WATER_ELEMENTAL_SPELL: u32 = 31_687;

pub fn equipment_needs_repair(condition: wow_state::inventory::EquipmentCondition) -> bool {
    condition.observed
        && (condition.broken_items > 0
            || condition
                .lowest_durability_percent
                .is_some_and(|percent| percent < REPAIR_DURABILITY_THRESHOLD_PERCENT))
}

fn catalog() -> &'static Catalog {
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../../data/buff-families.json"))
            .expect("embedded buff catalog must parse")
    })
}

fn glyph_catalog() -> &'static BTreeMap<u16, u32> {
    GLYPHS.get_or_init(|| {
        let catalog: GlyphCatalog =
            serde_json::from_str(include_str!("../../data/glyph-properties.json"))
                .expect("embedded glyph catalog must parse");
        catalog
            .glyphs
            .into_iter()
            .map(|glyph| (glyph.property_id, glyph.spell_id))
            .collect()
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MaintenanceDecision {
    Satisfied,
    Cast {
        family: String,
        spell: u32,
        target: EntityId,
    },
    SummonPet {
        spell: u32,
        player: EntityId,
    },
    EquipItem {
        item: u32,
        item_guid: EntityId,
        destination_slot: u8,
        player: EntityId,
    },
    UseItemInstance {
        item: u32,
        item_guid: EntityId,
        backpack_slot: u8,
        spell: u32,
        target: EntityId,
    },
    CastOnItem {
        spell: u32,
        item_guid: EntityId,
    },
    UseItemOnItem {
        item: u32,
        item_guid: EntityId,
        backpack_slot: u8,
        spell: u32,
        target_item_guid: EntityId,
    },
    VendorList {
        vendor: EntityId,
    },
    VendorBuy {
        vendor: EntityId,
        item: u32,
        slot: u32,
        lots: u32,
    },
    RecoveryVendorBuy {
        vendor: EntityId,
        item: u32,
        slot: u32,
        lots: u32,
    },
    TrainerList {
        trainer: EntityId,
    },
    TrainerBuy {
        trainer: EntityId,
        spell: u32,
    },
    QueryItem {
        item: u32,
    },
    PetReaction {
        pet: EntityId,
        reaction: u8,
    },
    PetAutocast {
        pet: EntityId,
        spell: u32,
        enabled: bool,
    },
    Deferred {
        family: String,
        reason: &'static str,
    },
}

pub fn decide_next(
    snapshot: &Snapshot,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
    include_party: bool,
) -> MaintenanceDecision {
    decide_next_with_nearby_services(snapshot, retry_after, now, include_party, None, None)
}

pub fn decide_next_with_nearby_vendor(
    snapshot: &Snapshot,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
    include_party: bool,
    nearby_sell_vendor: Option<EntityId>,
) -> MaintenanceDecision {
    decide_next_with_nearby_services(
        snapshot,
        retry_after,
        now,
        include_party,
        nearby_sell_vendor,
        None,
    )
}

pub fn decide_next_with_nearby_services(
    snapshot: &Snapshot,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
    include_party: bool,
    nearby_sell_vendor: Option<EntityId>,
    nearby_class_trainer: Option<EntityId>,
) -> MaintenanceDecision {
    let Some(class_id) = snapshot.state.capabilities.class_id else {
        return MaintenanceDecision::Deferred {
            family: "player_class".into(),
            reason: "class_not_authoritative",
        };
    };
    let Some(player_raw) = snapshot.state.session.character_guid else {
        return MaintenanceDecision::Deferred {
            family: "player".into(),
            reason: "player_guid_not_authoritative",
        };
    };
    let player = EntityId(player_raw);
    if !snapshot.state.session.in_world {
        return MaintenanceDecision::Deferred {
            family: "session".into(),
            reason: "not_in_world",
        };
    }
    if !snapshot.state.auras.by_entity.contains_key(&player) {
        return MaintenanceDecision::Deferred {
            family: "auras".into(),
            reason: "player_auras_not_authoritative",
        };
    }
    if snapshot
        .state
        .entities
        .0
        .get(&player)
        .is_some_and(wow_state::entities::EntityState::is_dead)
    {
        return MaintenanceDecision::Deferred {
            family: "life".into(),
            reason: "player_dead",
        };
    }
    if snapshot.state.control.mover.is_some() {
        return MaintenanceDecision::Deferred {
            family: "control".into(),
            reason: "controlled_mover_active",
        };
    }
    if snapshot.state.position.moving {
        return MaintenanceDecision::Deferred {
            family: "movement".into(),
            reason: "player_moving",
        };
    }
    if let Some(pet_decision) = hunter_pet_care(snapshot, class_id, player, retry_after, now) {
        return pet_decision;
    }
    if let Some(pet_decision) = death_knight_pet_care(snapshot, class_id, player, retry_after, now)
    {
        return pet_decision;
    }
    if let Some(pet_decision) =
        mage_water_elemental_care(snapshot, class_id, player, retry_after, now)
    {
        return pet_decision;
    }
    if let Some(pet_decision) = warlock_pet_care(snapshot, class_id, player, retry_after, now) {
        return pet_decision;
    }
    if let Some(pet_decision) = pet_setup(snapshot, retry_after, now) {
        return pet_decision;
    }
    if let Some(decision) = gear_upgrade(snapshot, player, retry_after, now) {
        return decision;
    }
    if class_id == 7
        && let Some(decision) = shaman_weapon_imbue(snapshot, retry_after, now)
    {
        return decision;
    }
    if class_id == 4
        && let Some(decision) = rogue_weapon_poison(snapshot, retry_after, now)
    {
        return decision;
    }
    if class_id == 4
        && let Some(decision) = rogue_poison_restock(snapshot, retry_after, now, nearby_sell_vendor)
    {
        return decision;
    }
    if let Some(decision) = spell_reagent_restock(snapshot, retry_after, now, nearby_sell_vendor) {
        return decision;
    }
    if class_id == 8
        && let Some(decision) = mage_supplies(snapshot, player, retry_after, now)
    {
        return decision;
    }
    if class_id == 9
        && let Some(decision) = warlock_stones(snapshot, player, retry_after, now)
    {
        return decision;
    }
    if let Some(decision) = recovery_supply_restock(snapshot, retry_after, now, nearby_sell_vendor)
    {
        return decision;
    }
    let player_position = snapshot
        .state
        .entities
        .0
        .get(&player)
        .and_then(|entity| entity.position)
        .or(snapshot.state.position.player);

    for family in catalog()
        .policies
        .iter()
        .filter(|policy| policy.class_id == class_id)
    {
        let Some(desired) = highest_known(snapshot, family) else {
            continue;
        };
        let spell = desired.spell;
        if !has_same_or_better(snapshot, player, family, desired.strength) {
            let Some(decision) = cast_decision(snapshot, family, spell, player, retry_after, now)
            else {
                continue;
            };
            return decision;
        }
        if include_party && family.party {
            for member in crate::group::state::online_members_except(snapshot, Some(player)) {
                if !snapshot.state.auras.by_entity.contains_key(&member) {
                    continue;
                }
                if !party_member_nearby(snapshot, player_position, member) {
                    continue;
                }
                if has_same_or_better(snapshot, member, family, desired.strength) {
                    continue;
                }
                if let Some(decision) =
                    cast_decision(snapshot, family, spell, member, retry_after, now)
                {
                    return decision;
                }
            }
        }
    }
    if let Some(decision) = class_training(snapshot, retry_after, now, nearby_class_trainer) {
        return decision;
    }
    MaintenanceDecision::Satisfied
}

fn class_training(
    snapshot: &Snapshot,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
    nearby_trainer: Option<EntityId>,
) -> Option<MaintenanceDecision> {
    let trainer = nearby_trainer?;
    let entity = snapshot.state.entities.0.get(&trainer)?;
    if entity.kind != wow_state::entities::EntityKind::Unit
        || !entity.interactable
        || !entity.npc_flags.is_some_and(is_class_trainer_flags)
    {
        return None;
    }
    let list_matches = snapshot.state.trainer.trainer == Some(trainer);
    if !list_matches {
        return (!retry_after
            .get(&(0, trainer))
            .is_some_and(|deadline| *deadline > now))
        .then_some(MaintenanceDecision::TrainerList { trainer });
    }
    if snapshot.state.trainer.trainer_type != Some(0) {
        return None;
    }
    if retry_after
        .get(&(u32::MAX, trainer))
        .is_some_and(|deadline| *deadline > now)
    {
        return None;
    }
    let player_level = class_training_player_level(snapshot)?;
    let mut offers = snapshot
        .state
        .trainer
        .offers
        .iter()
        .filter(|offer| {
            class_training_offer_is_eligible(snapshot, offer, player_level)
                && u64::from(offer.cost_copper)
                    .saturating_add(wow_domain::MAINTENANCE_PURCHASE_MONEY_RESERVE_COPPER)
                    <= snapshot.state.inventory.money
                && !retry_after
                    .get(&(offer.spell, trainer))
                    .is_some_and(|deadline| *deadline > now)
        })
        .collect::<Vec<_>>();
    offers.sort_by_key(|offer| {
        (
            std::cmp::Reverse(offer.required_level),
            offer.cost_copper,
            offer.spell,
        )
    });
    offers.first().map(|offer| MaintenanceDecision::TrainerBuy {
        trainer,
        spell: offer.spell,
    })
}

pub fn class_training_has_eligible_offer(snapshot: &Snapshot) -> bool {
    if snapshot.state.trainer.trainer_type != Some(0) {
        return false;
    }
    let Some(player_level) = class_training_player_level(snapshot) else {
        return false;
    };
    snapshot
        .state
        .trainer
        .offers
        .iter()
        .any(|offer| class_training_offer_is_eligible(snapshot, offer, player_level))
}

pub fn class_training_has_affordable_offer(snapshot: &Snapshot) -> bool {
    let Some(player_level) = class_training_player_level(snapshot) else {
        return false;
    };
    snapshot.state.trainer.trainer_type == Some(0)
        && snapshot.state.trainer.offers.iter().any(|offer| {
            class_training_offer_is_eligible(snapshot, offer, player_level)
                && u64::from(offer.cost_copper)
                    .saturating_add(wow_domain::MAINTENANCE_PURCHASE_MONEY_RESERVE_COPPER)
                    <= snapshot.state.inventory.money
        })
}

fn class_training_player_level(snapshot: &Snapshot) -> Option<u32> {
    snapshot
        .state
        .session
        .character_guid
        .map(EntityId)
        .and_then(|player| snapshot.state.entities.0.get(&player))
        .and_then(|player| player.level)
}

fn class_training_offer_is_eligible(
    snapshot: &Snapshot,
    offer: &wow_state::trainer::TrainerSpellOffer,
    player_level: u32,
) -> bool {
    offer.usable == 0
        && offer.spell != 0
        && u32::from(offer.required_level) <= player_level
        && !snapshot.state.capabilities.spells.contains(&offer.spell)
        && (offer.required_skill_line == 0
            || (snapshot.state.professions.known
                && u32::from(snapshot.state.professions.skill(offer.required_skill_line))
                    >= offer.required_skill_rank))
}

pub fn is_class_trainer_flags(flags: u32) -> bool {
    flags & 0x20 != 0 || (flags & 0x10 != 0 && flags & 0x40 == 0)
}

/// Keep the Mage's old ten item reserve using current inventory and spell state.
fn mage_supplies(
    snapshot: &Snapshot,
    player: EntityId,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
) -> Option<MaintenanceDecision> {
    let (food, drink) = mage_food_and_drink(snapshot)?;
    let ready_spell = |needle: &str| {
        snapshot
            .state
            .capabilities
            .spells
            .iter()
            .filter_map(|spell| {
                let metadata = crate::combat::spells::metadata(*spell)?;
                let name = metadata.name.to_ascii_lowercase();
                (name.contains(needle) || name.contains("conjure refreshment")).then_some(*spell)
            })
            .filter(|spell| {
                !retry_after
                    .get(&(*spell, player))
                    .is_some_and(|deadline| *deadline > now)
            })
            .filter(|spell| {
                crate::combat::readiness::check_spell_readiness(
                    snapshot,
                    *spell,
                    Some(player),
                    Millis::wall_clock_now().0,
                )
                .is_ok()
            })
            .max()
    };
    if drink < 10
        && let Some(spell) = ready_spell("conjure water")
    {
        return Some(MaintenanceDecision::Cast {
            family: "mage_water".into(),
            spell,
            target: player,
        });
    }
    if food < 10
        && let Some(spell) = ready_spell("conjure food")
    {
        return Some(MaintenanceDecision::Cast {
            family: "mage_food".into(),
            spell,
            target: player,
        });
    }
    None
}

/// Return None until all backpack item templates are authoritative.
fn mage_food_and_drink(snapshot: &Snapshot) -> Option<(u32, u32)> {
    let inventory = &snapshot.state.inventory;
    let mut food = 0u32;
    let mut drink = 0u32;
    for instance in inventory
        .instances
        .values()
        .filter(|instance| instance.backpack_slot >= 23 && instance.count > 0)
    {
        let item = inventory.item_metadata.get(&instance.item)?;
        let kinds = recovery_kinds(item);
        if kinds.food {
            food = food.saturating_add(instance.count);
        }
        if kinds.drink {
            drink = drink.saturating_add(instance.count);
        }
    }
    Some((food, drink))
}

/// Create missing Warlock stones, then prepare an available Soulstone on self.
fn warlock_stones(
    snapshot: &Snapshot,
    player: EntityId,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
) -> Option<MaintenanceDecision> {
    let has_healthstone = inventory_has_named_use_spell(snapshot, "healthstone");
    if !has_healthstone
        && let Some(spell) =
            highest_ready_named_spell(snapshot, player, "create healthstone", retry_after, now)
    {
        return Some(MaintenanceDecision::Cast {
            family: "warlock_create_healthstone".into(),
            spell,
            target: player,
        });
    }

    let soulstone = soulstone_instance(snapshot);
    if soulstone.is_none()
        && let Some(spell) =
            highest_ready_named_spell(snapshot, player, "create soulstone", retry_after, now)
    {
        return Some(MaintenanceDecision::Cast {
            family: "warlock_create_soulstone".into(),
            spell,
            target: player,
        });
    }

    let item = soulstone?;
    let spell = snapshot
        .state
        .inventory
        .item_metadata
        .get(&item.item)?
        .use_spell_id;
    if spell == 0 {
        return None;
    }
    if retry_after
        .get(&(spell, player))
        .is_some_and(|deadline| *deadline > now)
    {
        return None;
    }
    if snapshot
        .state
        .auras
        .by_entity
        .get(&player)?
        .values()
        .any(|aura| {
            aura.spell == spell
                || crate::combat::spells::metadata(aura.spell).is_some_and(|metadata| {
                    metadata
                        .name
                        .to_ascii_lowercase()
                        .contains("soulstone resurrection")
                })
        })
    {
        return None;
    }
    Some(MaintenanceDecision::UseItemInstance {
        item: item.item,
        item_guid: item.guid,
        backpack_slot: item.backpack_slot,
        spell,
        target: player,
    })
}

fn highest_ready_named_spell(
    snapshot: &Snapshot,
    player: EntityId,
    name: &str,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
) -> Option<u32> {
    snapshot
        .state
        .capabilities
        .spells
        .iter()
        .filter_map(|spell| {
            let metadata = crate::combat::spells::metadata(*spell)?;
            metadata
                .name
                .to_ascii_lowercase()
                .contains(name)
                .then_some(*spell)
        })
        .filter(|spell| {
            !retry_after
                .get(&(*spell, player))
                .is_some_and(|deadline| *deadline > now)
        })
        .filter(|spell| {
            crate::combat::readiness::check_spell_readiness(
                snapshot,
                *spell,
                Some(player),
                Millis::wall_clock_now().0,
            )
            .is_ok()
        })
        .max()
}

fn inventory_has_named_use_spell(snapshot: &Snapshot, name: &str) -> bool {
    snapshot
        .state
        .inventory
        .instances
        .values()
        .filter(|instance| instance.backpack_slot >= 23 && instance.count > 0)
        .any(|instance| {
            let Some(metadata) = snapshot.state.inventory.item_metadata.get(&instance.item) else {
                return false;
            };
            metadata.use_spell_id != 0
                && (metadata.name.to_ascii_lowercase().contains(name)
                    || crate::combat::spells::metadata(metadata.use_spell_id)
                        .is_some_and(|spell| spell.name.to_ascii_lowercase().contains(name)))
        })
}

fn soulstone_instance(snapshot: &Snapshot) -> Option<&wow_state::inventory::InventoryItemInstance> {
    snapshot
        .state
        .inventory
        .instances
        .values()
        .filter(|instance| instance.backpack_slot >= 23 && instance.count > 0)
        .filter(|instance| {
            let Some(metadata) = snapshot.state.inventory.item_metadata.get(&instance.item) else {
                return false;
            };
            metadata.use_spell_id != 0
                && (metadata.name.to_ascii_lowercase().contains("soulstone")
                    || crate::combat::spells::metadata(metadata.use_spell_id)
                        .is_some_and(|spell| spell.name.to_ascii_lowercase().contains("soulstone")))
        })
        .min_by_key(|instance| (instance.backpack_slot, instance.guid))
}

fn shaman_weapon_imbue(
    snapshot: &Snapshot,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
) -> Option<MaintenanceDecision> {
    let specialization = snapshot.state.capabilities.specialization_tree;
    let main_hand_names: &[&str] = match specialization {
        Some(0) => &["flametongue weapon"],
        Some(1) => &["windfury weapon", "flametongue weapon"],
        Some(2) => &["earthliving weapon", "flametongue weapon"],
        _ => &["flametongue weapon", "rockbiter weapon"],
    };
    let off_hand_names: &[&str] = if specialization == Some(1) {
        &["flametongue weapon"]
    } else {
        &[]
    };
    for (slot, names) in [(15, main_hand_names), (16, off_hand_names)] {
        let Some(item) = snapshot
            .state
            .inventory
            .equipped_item_instances
            .get(&slot)
            .filter(|item| item.temporary_enchanted == Some(false))
        else {
            continue;
        };
        for name in names {
            let spell = snapshot
                .state
                .capabilities
                .spells
                .iter()
                .filter_map(|spell| {
                    let metadata = crate::combat::spells::metadata(*spell)?;
                    metadata
                        .name
                        .to_ascii_lowercase()
                        .contains(name)
                        .then_some(*spell)
                })
                .filter(|spell| {
                    !retry_after
                        .get(&(*spell, item.guid))
                        .is_some_and(|deadline| *deadline > now)
                })
                .filter(|spell| {
                    crate::combat::readiness::check_spell_readiness(
                        snapshot,
                        *spell,
                        snapshot.state.session.character_guid.map(EntityId),
                        Millis::wall_clock_now().0,
                    )
                    .is_ok()
                })
                .max();
            if let Some(spell) = spell {
                return Some(MaintenanceDecision::CastOnItem {
                    spell,
                    item_guid: item.guid,
                });
            }
        }
    }
    None
}

fn rogue_weapon_poison(
    snapshot: &Snapshot,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
) -> Option<MaintenanceDecision> {
    // Keep the legacy PvE default behind this policy seam. Poison choice can
    // later vary by spec or user configuration without changing item actions.
    for (slot, family) in [(15, "instant poison"), (16, "deadly poison")] {
        let Some(weapon) = snapshot
            .state
            .inventory
            .equipped_item_instances
            .get(&slot)
            .filter(|weapon| weapon.temporary_enchanted == Some(false))
        else {
            continue;
        };
        let poison = snapshot
            .state
            .inventory
            .instances
            .values()
            .filter(|instance| instance.backpack_slot >= 23 && instance.count > 0)
            .filter_map(|instance| {
                let metadata = snapshot.state.inventory.item_metadata.get(&instance.item)?;
                let name = metadata.name.to_ascii_lowercase();
                let rank = poison_name_rank(&name, family)?;
                (metadata.item_class == 0 && metadata.subclass == 6 && metadata.use_spell_id != 0)
                    .then_some((rank, instance.item, instance, metadata.use_spell_id))
            })
            .max_by_key(|(rank, item, _, _)| (*rank, *item));
        let Some((_, item, instance, spell)) = poison else {
            continue;
        };
        if retry_after
            .get(&(spell, weapon.guid))
            .is_some_and(|deadline| *deadline > now)
        {
            continue;
        }
        return Some(MaintenanceDecision::UseItemOnItem {
            item,
            item_guid: instance.guid,
            backpack_slot: instance.backpack_slot,
            spell,
            target_item_guid: weapon.guid,
        });
    }
    None
}

fn poison_name_rank(name: &str, family: &str) -> Option<u8> {
    let suffix = name.strip_prefix(family)?.trim();
    if suffix.is_empty() {
        return Some(1);
    }
    match suffix {
        "ii" => Some(2),
        "iii" => Some(3),
        "iv" => Some(4),
        "v" => Some(5),
        "vi" => Some(6),
        "vii" => Some(7),
        "viii" => Some(8),
        "ix" => Some(9),
        _ => None,
    }
}

fn rogue_poison_restock(
    snapshot: &Snapshot,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
    nearby_sell_vendor: Option<EntityId>,
) -> Option<MaintenanceDecision> {
    const MINIMUM_POISON_COUNT: u32 = 5;
    let main_count = poison_count(snapshot, "instant poison");
    let off_count = poison_count(snapshot, "deadly poison");
    if main_count >= MINIMUM_POISON_COUNT && off_count >= MINIMUM_POISON_COUNT {
        return None;
    }

    let vendor = nearby_sell_vendor?;
    if snapshot.state.inventory.vendor != Some(vendor) {
        if retry_after
            .get(&(0, vendor))
            .is_some_and(|deadline| *deadline > now)
        {
            return None;
        }
        return Some(MaintenanceDecision::VendorList { vendor });
    }

    let inventory = snapshot
        .state
        .inventory
        .vendor_inventory
        .as_ref()
        .filter(|inventory| inventory.vendor == vendor);
    let Some(inventory) = inventory else {
        if retry_after
            .get(&(0, vendor))
            .is_some_and(|deadline| *deadline > now)
        {
            return None;
        }
        return Some(MaintenanceDecision::VendorList { vendor });
    };

    if let Some(offer) = inventory
        .offers
        .iter()
        .take(4)
        .filter(|offer| offer.extended_cost == 0)
        .find(|offer| {
            !snapshot
                .state
                .inventory
                .item_metadata
                .contains_key(&offer.item)
                && !retry_after
                    .get(&(offer.item, EntityId(0)))
                    .is_some_and(|deadline| *deadline > now)
        })
    {
        return Some(MaintenanceDecision::QueryItem { item: offer.item });
    }

    let level = snapshot
        .state
        .session
        .character_guid
        .map(EntityId)
        .and_then(|player| snapshot.state.entities.0.get(&player))
        .and_then(|player| player.level)
        .unwrap_or(0);
    let class_id = u32::from(snapshot.state.capabilities.class_id.unwrap_or_default());
    let spendable = snapshot
        .state
        .inventory
        .money
        .saturating_sub(wow_domain::MAINTENANCE_PURCHASE_MONEY_RESERVE_COPPER);
    let mut needed = [("instant poison", main_count), ("deadly poison", off_count)];
    needed.sort_by_key(|(_, count)| *count);
    for (family, count) in needed {
        if count >= MINIMUM_POISON_COUNT {
            continue;
        }
        let candidate = inventory
            .offers
            .iter()
            .filter(|offer| offer.extended_cost == 0 && offer.buy_count > 0)
            .filter(|offer| offer.stock.is_none_or(|stock| stock >= offer.buy_count))
            .filter(|offer| u64::from(offer.price_copper) <= spendable)
            .filter(|_| {
                !retry_after
                    .get(&(u32::MAX, vendor))
                    .is_some_and(|deadline| *deadline > now)
            })
            .filter_map(|offer| {
                let metadata = snapshot.state.inventory.item_metadata.get(&offer.item)?;
                let rank = poison_name_rank(&metadata.name.to_ascii_lowercase(), family)?;
                let class_allowed = metadata.allowable_class == 0
                    || metadata.allowable_class == u32::MAX
                    || ((1..=32).contains(&class_id)
                        && metadata.allowable_class & (1 << (class_id - 1)) != 0);
                (metadata.item_class == 0
                    && metadata.subclass == 6
                    && metadata.required_level <= level
                    && class_allowed)
                    .then_some((rank, metadata.required_level, offer.item, offer))
            })
            .max_by_key(|(rank, required_level, item, _)| (*rank, *required_level, *item));
        if let Some((_, _, item, offer)) = candidate {
            return Some(MaintenanceDecision::VendorBuy {
                vendor,
                item,
                slot: offer.slot,
                lots: wow_domain::MAX_MAINTENANCE_VENDOR_BUY_LOTS,
            });
        }
    }
    None
}

/// Refill known combat and class-utility spell reagents from an already nearby vendor.
fn spell_reagent_restock(
    snapshot: &Snapshot,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
    nearby_vendor: Option<EntityId>,
) -> Option<MaintenanceDecision> {
    const SOUL_SHARD: u32 = 6_265;
    const SOLO_USES: u32 = 5;
    const GROUP_USES: u32 = 10;

    let mut per_item = BTreeMap::<u32, u32>::new();
    for spell_id in &snapshot.state.capabilities.spells {
        let Some(spell) = crate::combat::spells::metadata(*spell_id) else {
            continue;
        };
        let spell_name = spell.name.to_ascii_lowercase();
        let automated_utility = spell.cost.base > 0
            || spell.cost.per_level > 0
            || spell.cost.percent > 0
            || spell.cost.per_second > 0
            || spell.cost.per_second_per_level > 0
            || spell.cost.use_all_power
            || spell.attack_spell
            || [
                "reincarnation",
                "soulstone",
                "ritual",
                "portal",
                "teleport",
                "rebirth",
                "divine intervention",
            ]
            .iter()
            .any(|needle| spell_name.contains(needle));
        if !automated_utility {
            continue;
        }
        for reagent in &spell.reagents {
            let Ok(item) = u32::try_from(reagent.item) else {
                continue;
            };
            if item == 0 || reagent.count == 0 || item == SOUL_SHARD {
                continue;
            }
            per_item
                .entry(item)
                .and_modify(|count| *count = (*count).max(reagent.count))
                .or_insert(reagent.count);
        }
    }
    if per_item.is_empty() {
        return None;
    }

    let grouped = snapshot.state.group.lifecycle != GroupLifecycle::Solo
        || !snapshot.state.group.members.is_empty();
    let desired_uses = if grouped { GROUP_USES } else { SOLO_USES };
    let mut low_reagents = per_item
        .into_iter()
        .map(|(item, per_cast)| {
            let count = snapshot.state.inventory.count(item);
            (item, per_cast, count, per_cast.saturating_mul(desired_uses))
        })
        .filter(|(_, _, count, target)| count < target)
        .collect::<Vec<_>>();
    low_reagents.sort_by_key(|(item, per_cast, count, _)| {
        (*count >= per_cast.saturating_mul(SOLO_USES), *count, *item)
    });
    if low_reagents.is_empty() {
        return None;
    }
    let vendor = nearby_vendor?;
    let inventory = &snapshot.state.inventory;
    let offers = inventory
        .vendor_inventory
        .as_ref()
        .filter(|offers| offers.vendor == vendor);
    if inventory.vendor != Some(vendor) || offers.is_none() {
        if retry_after
            .get(&(0, vendor))
            .is_some_and(|deadline| *deadline > now)
        {
            return None;
        }
        return Some(MaintenanceDecision::VendorList { vendor });
    }
    let offers = offers?;
    for (item, _, _, _) in &low_reagents {
        if !inventory.item_metadata.contains_key(item)
            && offers.offers.iter().any(|offer| {
                offer.item == *item
                    && !retry_after
                        .get(&(*item, EntityId(0)))
                        .is_some_and(|deadline| *deadline > now)
            })
        {
            return Some(MaintenanceDecision::QueryItem { item: *item });
        }
    }
    if retry_after
        .get(&(u32::MAX, vendor))
        .is_some_and(|deadline| *deadline > now)
    {
        return None;
    }
    let spendable = inventory
        .money
        .saturating_sub(wow_domain::MAINTENANCE_PURCHASE_MONEY_RESERVE_COPPER);
    let offer = low_reagents
        .iter()
        .flat_map(|(item, _, _, _)| {
            offers
                .offers
                .iter()
                .filter(move |offer| offer.item == *item)
        })
        .filter(|offer| inventory.item_metadata.contains_key(&offer.item))
        .filter(|offer| offer.extended_cost == 0 && offer.buy_count > 0)
        .filter(|offer| offer.stock.is_none_or(|stock| stock >= offer.buy_count))
        .filter(|offer| u64::from(offer.price_copper) <= spendable)
        .min_by_key(|offer| (offer.price_copper, offer.slot))?;
    Some(MaintenanceDecision::RecoveryVendorBuy {
        vendor,
        item: offer.item,
        slot: offer.slot,
        lots: wow_domain::MAX_MAINTENANCE_VENDOR_BUY_LOTS,
    })
}

fn poison_count(snapshot: &Snapshot, family: &str) -> u32 {
    snapshot
        .state
        .inventory
        .instances
        .values()
        .filter(|instance| instance.backpack_slot >= 23 && instance.count > 0)
        .filter_map(|instance| {
            let metadata = snapshot.state.inventory.item_metadata.get(&instance.item)?;
            (metadata.item_class == 0
                && metadata.subclass == 6
                && poison_name_rank(&metadata.name.to_ascii_lowercase(), family).is_some())
            .then_some(instance.count)
        })
        .fold(0_u32, u32::saturating_add)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecoverySupplyKind {
    Food,
    Drink,
    Bandage,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct RecoveryKinds {
    food: bool,
    drink: bool,
    bandage: bool,
}

fn recovery_kinds(metadata: &wow_state::inventory::ItemTemplateMetadata) -> RecoveryKinds {
    if metadata.item_class != 0 || metadata.use_spell_id == 0 {
        return RecoveryKinds::default();
    }
    match metadata.subclass {
        7 => RecoveryKinds {
            bandage: true,
            ..Default::default()
        },
        5 => {
            let name = metadata.name.to_ascii_lowercase();
            let is_drink = name.contains("water")
                || name.contains("drink")
                || name.contains("refreshment")
                || name.contains("strudel");
            let is_food = name.contains("food")
                || name.contains("bread")
                || name.contains("biscuit")
                || name.contains("jerky")
                || name.contains("meat")
                || name.contains("fish")
                || name.contains("cheese")
                || name.contains("strudel")
                || name.contains("refreshment");
            RecoveryKinds {
                food: is_food,
                drink: is_drink,
                bandage: false,
            }
        }
        _ => RecoveryKinds::default(),
    }
}

/// Check shared metadata eligibility for an automatically purchased recovery supply.
pub fn recovery_supply_item_is_eligible(
    metadata: &wow_state::inventory::ItemTemplateMetadata,
    class_id: u8,
    level: u32,
) -> bool {
    let class_allowed = metadata.allowable_class == 0
        || metadata.allowable_class == u32::MAX
        || ((1..=32).contains(&class_id) && metadata.allowable_class & (1 << (class_id - 1)) != 0);
    let kinds = recovery_kinds(metadata);
    (kinds.food || kinds.drink || kinds.bandage)
        && metadata.required_level <= level
        && class_allowed
}

fn matches_recovery_kind(kind: RecoverySupplyKind, kinds: RecoveryKinds) -> bool {
    match kind {
        RecoverySupplyKind::Food => kinds.food,
        RecoverySupplyKind::Drink => kinds.drink,
        RecoverySupplyKind::Bandage => kinds.bandage,
    }
}

fn recovery_supply_restock(
    snapshot: &Snapshot,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
    nearby_vendor: Option<EntityId>,
) -> Option<MaintenanceDecision> {
    const TARGET_COUNT: u32 = 10;
    if snapshot.state.capabilities.class_id == Some(8) {
        return None;
    }
    let player = EntityId(snapshot.state.session.character_guid?);
    let inventory = &snapshot.state.inventory;

    // Unknown backpack templates can hide existing recovery items. Wait until
    // the inventory facts are complete before treating a category as low.
    if inventory.instances.values().any(|instance| {
        instance.count > 0
            && instance.backpack_slot >= 23
            && !inventory.item_metadata.contains_key(&instance.item)
    }) {
        return None;
    }
    let mut counts = [0_u32; 3];
    for instance in inventory
        .instances
        .values()
        .filter(|instance| instance.count > 0 && instance.backpack_slot >= 23)
    {
        if let Some(metadata) = inventory.item_metadata.get(&instance.item) {
            let kinds = recovery_kinds(metadata);
            for (index, matches) in [kinds.food, kinds.drink, kinds.bandage]
                .into_iter()
                .enumerate()
            {
                if matches {
                    counts[index] = counts[index].saturating_add(instance.count);
                }
            }
        }
    }
    let low_kinds: Vec<_> = [
        RecoverySupplyKind::Food,
        RecoverySupplyKind::Drink,
        RecoverySupplyKind::Bandage,
    ]
    .into_iter()
    .enumerate()
    .filter_map(|(index, kind)| (counts[index] < TARGET_COUNT).then_some((kind, counts[index])))
    .collect();
    if low_kinds.is_empty() {
        return None;
    }
    let vendor = nearby_vendor?;
    let offer_list = inventory
        .vendor_inventory
        .as_ref()
        .filter(|offers| offers.vendor == vendor);
    if inventory.vendor != Some(vendor) || offer_list.is_none() {
        if retry_after
            .get(&(0, vendor))
            .is_some_and(|deadline| *deadline > now)
        {
            return None;
        }
        return Some(MaintenanceDecision::VendorList { vendor });
    }
    let offer_list = offer_list?;
    let bandage_evidence = inventory.instances.values().any(|instance| {
        instance.count > 0
            && instance.backpack_slot >= 23
            && inventory
                .item_metadata
                .get(&instance.item)
                .is_some_and(|metadata| recovery_kinds(metadata).bandage)
    }) || offer_list.offers.iter().any(|offer| {
        inventory
            .item_metadata
            .get(&offer.item)
            .is_some_and(|metadata| recovery_kinds(metadata).bandage)
    });
    if let Some(offer) = offer_list.offers.iter().find(|offer| {
        offer.extended_cost == 0
            && offer.buy_count > 0
            && !inventory.item_metadata.contains_key(&offer.item)
            && !retry_after
                .get(&(offer.item, EntityId(0)))
                .is_some_and(|deadline| *deadline > now)
    }) {
        return Some(MaintenanceDecision::QueryItem { item: offer.item });
    }
    let spendable = inventory
        .money
        .saturating_sub(wow_domain::MAINTENANCE_PURCHASE_MONEY_RESERVE_COPPER);
    let level = snapshot
        .state
        .entities
        .0
        .get(&player)
        .and_then(|entity| entity.level)
        .unwrap_or(0);
    let class = snapshot.state.capabilities.class_id.unwrap_or_default();
    if retry_after
        .get(&(u32::MAX, vendor))
        .is_some_and(|deadline| *deadline > now)
    {
        return None;
    }
    for (kind, _) in low_kinds {
        if kind == RecoverySupplyKind::Bandage && !bandage_evidence {
            continue;
        }
        let candidate = offer_list
            .offers
            .iter()
            .filter(|offer| offer.extended_cost == 0 && offer.buy_count > 0)
            .filter(|offer| offer.stock.is_none_or(|stock| stock >= offer.buy_count))
            .filter(|offer| u64::from(offer.price_copper) <= spendable)
            .filter_map(|offer| {
                let metadata = inventory.item_metadata.get(&offer.item)?;
                (matches_recovery_kind(kind, recovery_kinds(metadata))
                    && recovery_supply_item_is_eligible(metadata, class, level))
                .then_some((
                    metadata.required_level,
                    metadata.item_level,
                    offer.item,
                    offer,
                ))
            })
            .max_by_key(|(required_level, item_level, item, _)| {
                (*required_level, *item_level, *item)
            });
        if let Some((_, _, item, offer)) = candidate {
            return Some(MaintenanceDecision::RecoveryVendorBuy {
                vendor,
                item,
                slot: offer.slot,
                lots: wow_domain::MAX_MAINTENANCE_VENDOR_BUY_LOTS,
            });
        }
    }
    None
}

/// Apply the old pet defaults after SMSG_PET_SPELLS confirms a control bar.
fn pet_setup(
    snapshot: &Snapshot,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
) -> Option<MaintenanceDecision> {
    let pet = snapshot.state.pet.guid?;
    let reaction = snapshot.state.pet.reaction?;
    if reaction != 1
        && !retry_after
            .get(&(0, pet))
            .is_some_and(|deadline| *deadline > now)
    {
        return Some(MaintenanceDecision::PetReaction { pet, reaction: 1 });
    }
    let grouped = snapshot.state.group.lifecycle != GroupLifecycle::Solo
        || !snapshot.state.group.members.is_empty();
    const ALWAYS_MANUAL: &[u32] = &[
        1742, 24450, 24452, 24453, 47482, 19244, 19647, 19505, 19731, 19734, 19736, 27276, 27277,
        48011, 58867,
    ];
    const TAUNTS: &[u32] = &[2649, 3716, 17735, 33698];
    snapshot.state.pet.abilities.iter().find_map(|ability| {
        let current = ability.autocast?;
        let desired = if ALWAYS_MANUAL.contains(&ability.spell) {
            false
        } else if TAUNTS.contains(&ability.spell) {
            !grouped
        } else {
            true
        };
        (current != desired
            && !retry_after
                .get(&(ability.spell, pet))
                .is_some_and(|deadline| *deadline > now))
        .then_some(MaintenanceDecision::PetAutocast {
            pet,
            spell: ability.spell,
            enabled: desired,
        })
    })
}

fn gear_upgrade(
    snapshot: &Snapshot,
    player: EntityId,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
) -> Option<MaintenanceDecision> {
    use wow_state::inventory::ItemTemplateMetadata;
    let inventory = &snapshot.state.inventory;
    if !inventory.equipment_slots_authoritative {
        return None;
    }
    let level = snapshot.state.entities.0.get(&player)?.level?;
    let class = snapshot.state.capabilities.class_id?;
    let score = |metadata: &ItemTemplateMetadata| {
        crate::gear::item_score_with_spec(
            metadata,
            class,
            snapshot.state.capabilities.specialization_tree,
        )
    };
    let mut best: Option<(f32, u32, EntityId, u8)> = None;
    if let Some(destination_slot) =
        (19u8..=22).find(|slot| !inventory.equipped_items.contains_key(slot))
    {
        if let Some(candidate) = inventory
            .instances
            .values()
            .filter(|instance| instance.backpack_slot >= 23 && instance.count > 0)
            .filter_map(|instance| {
                let metadata = inventory.item_metadata.get(&instance.item)?;
                (crate::gear::container_can_equip(metadata, class, level.min(255) as u8)
                    && !retry_after
                        .get(&(instance.item, player))
                        .is_some_and(|deadline| *deadline > now))
                .then_some((metadata.container_slots, instance))
            })
            .max_by_key(|(slots, _)| *slots)
        {
            return Some(MaintenanceDecision::EquipItem {
                item: candidate.1.item,
                item_guid: candidate.1.guid,
                destination_slot,
                player,
            });
        }
    }
    if inventory
        .equipped_items
        .values()
        .any(|item| *item != 0 && !inventory.item_metadata.contains_key(item))
    {
        return None;
    }
    for instance in inventory
        .instances
        .values()
        .filter(|instance| instance.backpack_slot >= 23 && instance.count > 0)
    {
        let Some(metadata) = inventory.item_metadata.get(&instance.item) else {
            continue;
        };
        if !crate::gear::player_can_use(metadata, class, level as u8) {
            continue;
        }
        let slots = crate::gear::destination_slots(metadata);
        let new_score = score(metadata);
        for slot in slots {
            if *slot == 16
                && inventory
                    .equipped_items
                    .get(&15)
                    .and_then(|item| inventory.item_metadata.get(item))
                    .is_some_and(|item| item.inventory_type == 17)
            {
                continue;
            }
            let mut old_score = inventory
                .equipped_items
                .get(slot)
                .and_then(|item| inventory.item_metadata.get(item))
                .map(score)
                .unwrap_or(0.0);
            if metadata.inventory_type == 17 && *slot == 15 {
                old_score += inventory
                    .equipped_items
                    .get(&16)
                    .and_then(|item| inventory.item_metadata.get(item))
                    .map(score)
                    .unwrap_or(0.0);
            }
            let delta = new_score - old_score;
            if !crate::gear::score_improves_by_fraction(new_score, old_score, 0.01)
                || retry_after
                    .get(&(instance.item, player))
                    .is_some_and(|deadline| *deadline > now)
            {
                continue;
            }
            if best.as_ref().is_none_or(|(known, ..)| delta > *known) {
                best = Some((delta, instance.item, instance.guid, *slot));
            }
        }
    }
    best.map(
        |(_, item, item_guid, destination_slot)| MaintenanceDecision::EquipItem {
            item,
            item_guid,
            destination_slot,
            player,
        },
    )
}

/// Restore a hunter's confirmed missing or dead pet before ordinary buffs.
/// Unknown control state and an unobserved pet entity remain fail-closed.
fn hunter_pet_care(
    snapshot: &Snapshot,
    class_id: u8,
    player: EntityId,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
) -> Option<MaintenanceDecision> {
    if class_id != 3 || !snapshot.state.pet.control_known {
        return None;
    }
    let (spell, family) = match snapshot.state.pet.guid {
        None => (883, "call_pet"),
        Some(pet) => {
            let entity = snapshot.state.entities.0.get(&pet)?;
            if !entity.is_dead() {
                return None;
            }
            (982, "revive_pet")
        }
    };
    if retry_after
        .get(&(spell, player))
        .is_some_and(|deadline| *deadline > now)
    {
        return None;
    }
    crate::combat::readiness::check_spell_readiness(
        snapshot,
        spell,
        Some(player),
        Millis::wall_clock_now().0,
    )
    .ok()?;
    Some(MaintenanceDecision::Cast {
        family: family.into(),
        spell,
        target: player,
    })
}

/// Restore an absent or dead Warlock demon before ordinary buffs.
fn warlock_pet_care(
    snapshot: &Snapshot,
    class_id: u8,
    player: EntityId,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
) -> Option<MaintenanceDecision> {
    if class_id != 9 {
        return None;
    }
    if let Some(pet) = snapshot.state.pet.guid {
        let entity = snapshot.state.entities.0.get(&pet)?;
        if !entity.is_dead() {
            return None;
        }
    }

    if WARLOCK_PET_SUMMONS.iter().any(|spell| {
        retry_after
            .get(&(*spell, player))
            .is_some_and(|deadline| *deadline > now)
    }) {
        return None;
    }

    let spell = preferred_warlock_demon_spells(snapshot)
        .into_iter()
        .find(|spell| {
            snapshot.state.capabilities.spells.contains(spell)
                && crate::combat::readiness::check_spell_readiness(
                    snapshot,
                    **spell,
                    None,
                    Millis::wall_clock_now().0,
                )
                .is_ok()
        })?;

    Some(MaintenanceDecision::SummonPet {
        spell: *spell,
        player,
    })
}

const WARLOCK_PET_SUMMONS: [u32; 5] = [688, 697, 712, 691, 30146];
const DEATH_KNIGHT_PERSISTENT_PET: u32 = 46584;

pub fn is_warlock_persistent_demon_summon(spell: u32) -> bool {
    WARLOCK_PET_SUMMONS.contains(&spell)
}

pub fn is_persistent_pet_summon(spell: u32) -> bool {
    is_warlock_persistent_demon_summon(spell)
        || spell == DEATH_KNIGHT_PERSISTENT_PET
        || spell == MAGE_WATER_ELEMENTAL_SPELL
}

/// Restore a Death Knight's permanent ghoul only with Master of Ghouls.
fn death_knight_pet_care(
    snapshot: &Snapshot,
    class_id: u8,
    player: EntityId,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
) -> Option<MaintenanceDecision> {
    if class_id != 6
        || !snapshot
            .state
            .capabilities
            .active_talents
            .iter()
            .any(|talent| talent.talent_id == 1984)
        || !snapshot
            .state
            .capabilities
            .spells
            .contains(&DEATH_KNIGHT_PERSISTENT_PET)
    {
        return None;
    }
    persistent_pet_summon(
        snapshot,
        player,
        DEATH_KNIGHT_PERSISTENT_PET,
        retry_after,
        now,
    )
}

/// Restore the Eternal Water elemental only when its glyph is proven active.
fn mage_water_elemental_care(
    snapshot: &Snapshot,
    class_id: u8,
    player: EntityId,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
) -> Option<MaintenanceDecision> {
    if class_id != 8
        || !snapshot
            .state
            .capabilities
            .spells
            .contains(&MAGE_WATER_ELEMENTAL_SPELL)
        || !active_glyph_spell_ids(snapshot)?.contains(&ETERNAL_WATER_GLYPH_SPELL)
    {
        return None;
    }
    persistent_pet_summon(
        snapshot,
        player,
        MAGE_WATER_ELEMENTAL_SPELL,
        retry_after,
        now,
    )
}

fn active_glyph_spell_ids(snapshot: &Snapshot) -> Option<Vec<u32>> {
    snapshot
        .state
        .capabilities
        .active_glyph_properties
        .as_ref()?
        .iter()
        .map(|property| glyph_catalog().get(property).copied())
        .collect()
}

fn persistent_pet_summon(
    snapshot: &Snapshot,
    player: EntityId,
    spell: u32,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
) -> Option<MaintenanceDecision> {
    if let Some(pet) = snapshot.state.pet.guid {
        let entity = snapshot.state.entities.0.get(&pet)?;
        if !entity.is_dead() {
            return None;
        }
    }
    if retry_after
        .get(&(spell, player))
        .is_some_and(|deadline| *deadline > now)
    {
        return None;
    }
    crate::combat::readiness::check_spell_readiness(
        snapshot,
        spell,
        None,
        Millis::wall_clock_now().0,
    )
    .ok()?;
    Some(MaintenanceDecision::SummonPet { spell, player })
}

fn preferred_warlock_demon_spells(snapshot: &Snapshot) -> &'static [u32] {
    const DEMONOLOGY: &[u32] = &[30146, 697, 688];
    const DESTRUCTION: &[u32] = &[688, 691];
    const AFFLICTION_GROUP: &[u32] = &[691, 688];
    const AFFLICTION_SOLO: &[u32] = &[697, 688];
    const UNKNOWN_SPEC: &[u32] = &[688, 697];
    let grouped = snapshot.state.group.lifecycle != GroupLifecycle::Solo
        || !snapshot.state.group.members.is_empty();
    match snapshot.state.capabilities.specialization_tree {
        Some(1) => DEMONOLOGY,
        Some(2) => DESTRUCTION,
        Some(0) if grouped => AFFLICTION_GROUP,
        Some(0) => AFFLICTION_SOLO,
        _ => UNKNOWN_SPEC,
    }
}

fn cast_decision(
    snapshot: &Snapshot,
    family: &BuffFamilyPolicy,
    spell: u32,
    target: EntityId,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
) -> Option<MaintenanceDecision> {
    if retry_after
        .get(&(spell, target))
        .is_some_and(|deadline| *deadline > now)
    {
        return None;
    }
    if let Err(reason) = crate::combat::readiness::check_spell_readiness(
        snapshot,
        spell,
        Some(target),
        Millis::wall_clock_now().0,
    ) {
        return Some(MaintenanceDecision::Deferred {
            family: family.family.clone(),
            reason: readiness_reason(reason),
        });
    }
    Some(MaintenanceDecision::Cast {
        family: family.family.clone(),
        spell,
        target,
    })
}

fn readiness_reason(reason: crate::combat::readiness::SpellUnavailableReason) -> &'static str {
    use crate::combat::readiness::SpellUnavailableReason::*;
    match reason {
        Unknown => "spell_unknown",
        UnknownMetadata => "spell_metadata_unknown",
        UnknownState => "required_state_unknown",
        WrongClass => "spell_wrong_class",
        Cooldown => "spell_cooldown",
        GlobalCooldown => "global_cooldown",
        CastInProgress => "cast_in_progress",
        InsufficientPower => "insufficient_power",
        InsufficientRunes => "insufficient_runes",
        MissingComboPoints => "missing_combo_points",
        MissingReagent => "missing_reagent",
        MissingEquipment => "missing_equipment",
        RequirementNotMet => "spell_requirement_not_met",
        UnsupportedRequirement => "spell_requirement_unsupported",
        TargetNotAuthoritative => "target_not_authoritative",
        NotInWorld => "not_in_world",
    }
}

pub fn retry_deadline(now: Instant) -> Instant {
    now + Duration::from_secs(5)
}

fn highest_known<'a>(snapshot: &Snapshot, family: &'a BuffFamilyPolicy) -> Option<&'a RankedSpell> {
    family
        .spells
        .iter()
        .filter(|ranked| snapshot.state.capabilities.spells.contains(&ranked.spell))
        .max_by_key(|ranked| (ranked.strength, ranked.level, ranked.rank, ranked.spell))
}

fn has_same_or_better(
    snapshot: &Snapshot,
    target: EntityId,
    family: &BuffFamilyPolicy,
    desired_strength: u32,
) -> bool {
    let active = snapshot.state.auras.spells(target);
    family
        .spells
        .iter()
        .any(|ranked| active.contains(&ranked.spell) && ranked.strength >= desired_strength)
}

fn party_member_nearby(
    snapshot: &Snapshot,
    player_pos: Option<WorldPosition>,
    member: EntityId,
) -> bool {
    let member_pos = snapshot
        .state
        .entities
        .0
        .get(&member)
        .and_then(|entity| entity.position);
    player_pos
        .zip(member_pos)
        .is_some_and(|(player_pos, member_pos)| {
            player_pos.map == member_pos.map && player_pos.point.distance(member_pos.point) <= 20.0
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_requires_observed_broken_or_yellow_equipment() {
        assert!(!equipment_needs_repair(
            wow_state::inventory::EquipmentCondition::default()
        ));
        assert!(!equipment_needs_repair(
            wow_state::inventory::EquipmentCondition {
                observed: true,
                lowest_durability_percent: Some(25),
                broken_items: 0,
            }
        ));
        assert!(equipment_needs_repair(
            wow_state::inventory::EquipmentCondition {
                observed: true,
                lowest_durability_percent: Some(24),
                broken_items: 0,
            }
        ));
        assert!(equipment_needs_repair(
            wow_state::inventory::EquipmentCondition {
                observed: true,
                lowest_durability_percent: None,
                broken_items: 1,
            }
        ));
    }

    fn recovery_vendor_state() -> (AuthoritativeState, EntityId) {
        let vendor = EntityId(55);
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(1);
        state.inventory.money = 5_000;
        state.inventory.vendor = Some(vendor);
        state.inventory.vendor_inventory = Some(wow_state::inventory::VendorInventory {
            vendor,
            offers: vec![wow_state::inventory::VendorOffer {
                slot: 2,
                item: 117,
                stock: Some(10),
                price_copper: 100,
                buy_count: 5,
                extended_cost: 0,
            }],
        });
        state.inventory.item_metadata.insert(
            117,
            wow_state::inventory::ItemTemplateMetadata {
                name: "Tough Jerky".into(),
                item_class: 0,
                subclass: 5,
                use_spell_id: 1,
                allowable_class: 0,
                required_level: 1,
                ..Default::default()
            },
        );
        state.entities.0.insert(
            EntityId(7),
            wow_state::entities::EntityState {
                level: Some(10),
                ..Default::default()
            },
        );
        (state, vendor)
    }

    fn reagent_vendor_state() -> (AuthoritativeState, EntityId) {
        let vendor = EntityId(77);
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(8);
        state.capabilities.spells.insert(130); // Slow Fall uses one Light Feather.
        state.inventory.money = 5_000;
        state.inventory.vendor = Some(vendor);
        state.inventory.vendor_inventory = Some(wow_state::inventory::VendorInventory {
            vendor,
            offers: vec![wow_state::inventory::VendorOffer {
                slot: 3,
                item: 17_056,
                stock: Some(20),
                price_copper: 100,
                buy_count: 5,
                extended_cost: 0,
            }],
        });
        state.inventory.item_metadata.insert(
            17_056,
            wow_state::inventory::ItemTemplateMetadata {
                name: "Light Feather".into(),
                ..Default::default()
            },
        );
        (state, vendor)
    }

    #[test]
    fn reagent_restock_uses_known_spell_cost_and_one_affordable_vendor_lot() {
        let (state, vendor) = reagent_vendor_state();
        assert_eq!(
            spell_reagent_restock(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                Some(vendor),
            ),
            Some(MaintenanceDecision::RecoveryVendorBuy {
                vendor,
                item: 17_056,
                slot: 3,
                lots: wow_domain::MAX_MAINTENANCE_VENDOR_BUY_LOTS,
            })
        );
    }

    #[test]
    fn reagent_restock_is_local_advisory_and_keeps_the_money_reserve() {
        let (mut state, vendor) = reagent_vendor_state();
        assert_eq!(
            spell_reagent_restock(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                None
            ),
            None,
            "a reagent shortage must not trigger travel"
        );
        state.inventory.money = 1_099;
        assert_eq!(
            spell_reagent_restock(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                Some(vendor),
            ),
            None,
            "the purchase must preserve 1,000 copper"
        );
        state.inventory.money = 5_000;
        state.inventory.items.insert(17_056, 5);
        assert_eq!(
            spell_reagent_restock(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                Some(vendor),
            ),
            None,
            "five casts of the reagent satisfy the solo reserve"
        );
        state.group.lifecycle = GroupLifecycle::Active;
        assert_eq!(
            spell_reagent_restock(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                Some(vendor),
            )
            .map(|decision| match decision {
                MaintenanceDecision::RecoveryVendorBuy { item, .. } => item,
                _ => 0,
            }),
            Some(17_056),
            "grouped play keeps ten casts when practical"
        );
        state.inventory.items.insert(17_056, 10);
        assert_eq!(
            spell_reagent_restock(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                Some(vendor),
            ),
            None
        );
    }

    #[test]
    fn reagent_restock_rejects_unsafe_offers_and_soul_shards() {
        let (mut state, vendor) = reagent_vendor_state();
        let now = Instant::now();
        for (stock, extended_cost) in [(Some(4), 0), (Some(20), 1)] {
            let offer = &mut state.inventory.vendor_inventory.as_mut().unwrap().offers[0];
            offer.stock = stock;
            offer.extended_cost = extended_cost;
            assert_eq!(
                spell_reagent_restock(
                    &Snapshot::from_state(&state),
                    &BTreeMap::new(),
                    now,
                    Some(vendor),
                ),
                None
            );
        }
        state.capabilities.class_id = Some(9);
        state.capabilities.spells.clear();
        state.capabilities.spells.insert(691); // Summon Felhunter consumes a Soul Shard.
        state.inventory.vendor_inventory.as_mut().unwrap().offers[0].item = 6_265;
        state.inventory.item_metadata.insert(
            6_265,
            wow_state::inventory::ItemTemplateMetadata {
                name: "Soul Shard".into(),
                ..Default::default()
            },
        );
        assert_eq!(
            spell_reagent_restock(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                now,
                Some(vendor),
            ),
            None,
            "Soul Shards are created from combat and must not be bought"
        );
    }

    #[test]
    fn reagent_restock_queries_unknown_offer_templates_before_buying() {
        let (mut state, vendor) = reagent_vendor_state();
        state.inventory.item_metadata.remove(&17_056);
        assert_eq!(
            spell_reagent_restock(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                Some(vendor),
            ),
            Some(MaintenanceDecision::QueryItem { item: 17_056 })
        );
        let retrying = [(
            (17_056, EntityId(0)),
            Instant::now() + Duration::from_secs(5),
        )]
        .into_iter()
        .collect();
        assert_eq!(
            spell_reagent_restock(
                &Snapshot::from_state(&state),
                &retrying,
                Instant::now(),
                Some(vendor),
            ),
            None,
            "an item metadata query in backoff does not authorize purchase"
        );
    }

    #[test]
    fn recovery_restock_buys_one_affordable_lot_from_observed_nearby_offer() {
        let (state, vendor) = recovery_vendor_state();
        let decision = recovery_supply_restock(
            &Snapshot::from_state(&state),
            &BTreeMap::new(),
            Instant::now(),
            Some(vendor),
        );
        assert_eq!(
            decision,
            Some(MaintenanceDecision::RecoveryVendorBuy {
                vendor,
                item: 117,
                slot: 2,
                lots: 1,
            })
        );

        let mut state = state;
        state.inventory.money = 1_099;
        assert_eq!(
            recovery_supply_restock(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                Some(vendor),
            ),
            None,
            "purchase must preserve the 1,000-copper reserve"
        );
    }

    #[test]
    fn recovery_restock_rejects_unsafe_or_incomplete_vendor_evidence() {
        let (mut state, vendor) = recovery_vendor_state();
        let snapshot = Snapshot::from_state(&state);
        assert_eq!(
            recovery_supply_restock(&snapshot, &BTreeMap::new(), Instant::now(), None),
            None,
            "no vendor means no travel or purchase"
        );

        state.inventory.vendor_inventory.as_mut().unwrap().offers[0].stock = Some(4);
        assert_eq!(
            recovery_supply_restock(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                Some(vendor),
            ),
            None,
            "partial stock cannot satisfy one lot"
        );
        state.inventory.vendor_inventory.as_mut().unwrap().offers[0].stock = Some(10);
        state.inventory.vendor_inventory.as_mut().unwrap().offers[0].extended_cost = 1;
        assert_eq!(
            recovery_supply_restock(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                Some(vendor),
            ),
            None,
            "extended currency offers are rejected"
        );
        state.inventory.vendor_inventory.as_mut().unwrap().offers[0].extended_cost = 0;

        state.inventory.instances.insert(
            EntityId(91),
            wow_state::inventory::InventoryItemInstance {
                item: 999,
                guid: EntityId(91),
                backpack_slot: 23,
                count: 1,
            },
        );
        assert_eq!(
            recovery_supply_restock(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                Some(vendor),
            ),
            None,
            "unknown backpack metadata blocks purchases"
        );
    }

    #[test]
    fn refreshment_counts_as_both_food_and_drink_for_shared_maintenance() {
        let mut state = AuthoritativeState::default();
        state.inventory.instances.insert(
            EntityId(81),
            wow_state::inventory::InventoryItemInstance {
                item: 456,
                guid: EntityId(81),
                backpack_slot: 23,
                count: 4,
            },
        );
        state.inventory.item_metadata.insert(
            456,
            wow_state::inventory::ItemTemplateMetadata {
                name: "Conjured Refreshment".into(),
                item_class: 0,
                subclass: 5,
                use_spell_id: 1,
                ..Default::default()
            },
        );
        assert_eq!(
            mage_food_and_drink(&Snapshot::from_state(&state)),
            Some((4, 4))
        );
    }

    #[test]
    fn bandage_offer_establishes_a_restock_need_when_backpack_is_complete() {
        let (mut state, vendor) = recovery_vendor_state();
        state
            .inventory
            .vendor_inventory
            .as_mut()
            .unwrap()
            .offers
            .push(wow_state::inventory::VendorOffer {
                slot: 3,
                item: 118,
                stock: Some(10),
                price_copper: 100,
                buy_count: 1,
                extended_cost: 0,
            });
        state.inventory.item_metadata.insert(
            118,
            wow_state::inventory::ItemTemplateMetadata {
                name: "Linen Bandage".into(),
                item_class: 0,
                subclass: 7,
                use_spell_id: 1,
                allowable_class: 0,
                required_level: 1,
                ..Default::default()
            },
        );
        for (item, name, spell) in [(201, "Conjured Food", 1), (202, "Conjured Water", 2)] {
            state.inventory.instances.insert(
                EntityId(item as u64),
                wow_state::inventory::InventoryItemInstance {
                    item,
                    guid: EntityId(item as u64),
                    backpack_slot: 23,
                    count: 10,
                },
            );
            state.inventory.item_metadata.insert(
                item,
                wow_state::inventory::ItemTemplateMetadata {
                    name: name.into(),
                    item_class: 0,
                    subclass: 5,
                    use_spell_id: spell,
                    ..Default::default()
                },
            );
        }
        assert_eq!(
            recovery_supply_restock(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                Some(vendor),
            ),
            Some(MaintenanceDecision::RecoveryVendorBuy {
                vendor,
                item: 118,
                slot: 3,
                lots: 1,
            })
        );
    }

    #[test]
    fn shaman_applies_highest_known_spec_weapon_imbue_only_to_observed_unenchanted_weapon() {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(7);
        state.capabilities.specialization_tree = Some(0);
        state.capabilities.spells.extend([8024, 16342]);
        state.inventory.equipped_item_instances.insert(
            15,
            wow_state::inventory::EquippedItemInstance {
                item: 19019,
                guid: EntityId(19),
                temporary_enchanted: Some(false),
            },
        );
        authoritative_caster(&mut state, EntityId(7));
        state.auras.by_entity.entry(EntityId(7)).or_default();
        state.inventory.equipment_slots_authoritative = true;
        state.inventory.equipped_items.insert(15, 19019);

        assert_eq!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false,
            ),
            MaintenanceDecision::CastOnItem {
                spell: 16342,
                item_guid: EntityId(19),
            }
        );

        state
            .inventory
            .equipped_item_instances
            .get_mut(&15)
            .unwrap()
            .temporary_enchanted = Some(true);
        assert_eq!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false,
            ),
            MaintenanceDecision::Satisfied
        );
    }

    #[test]
    fn rogue_applies_highest_inventory_poison_to_each_unenchanted_weapon() {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(4);
        state.inventory.equipped_item_instances.insert(
            15,
            wow_state::inventory::EquippedItemInstance {
                item: 19019,
                guid: EntityId(19),
                temporary_enchanted: Some(false),
            },
        );
        state.inventory.equipped_item_instances.insert(
            16,
            wow_state::inventory::EquippedItemInstance {
                item: 19019,
                guid: EntityId(20),
                temporary_enchanted: Some(false),
            },
        );
        for (item, guid, name) in [
            (100, EntityId(100), "Instant Poison V"),
            (101, EntityId(101), "Instant Poison IX"),
            (102, EntityId(102), "Deadly Poison VIII"),
        ] {
            state.inventory.instances.insert(
                guid,
                InventoryItemInstance {
                    item,
                    guid,
                    backpack_slot: 23 + (item - 100) as u8,
                    count: 1,
                },
            );
            state.inventory.item_metadata.insert(
                item,
                ItemTemplateMetadata {
                    name: name.into(),
                    item_class: 0,
                    subclass: 6,
                    use_spell_id: 5000 + item,
                    ..Default::default()
                },
            );
        }
        authoritative_caster(&mut state, EntityId(7));
        state.auras.by_entity.entry(EntityId(7)).or_default();

        let snapshot = Snapshot::from_state(&state);
        assert_eq!(
            decide_next(&snapshot, &BTreeMap::new(), Instant::now(), false),
            MaintenanceDecision::UseItemOnItem {
                item: 101,
                item_guid: EntityId(101),
                backpack_slot: 24,
                spell: 5101,
                target_item_guid: EntityId(19),
            }
        );
        let retry = [(5101, EntityId(19))]
            .into_iter()
            .map(|key| (key, Instant::now() + Duration::from_secs(20)))
            .collect();
        assert_eq!(
            decide_next(&snapshot, &retry, Instant::now(), false),
            MaintenanceDecision::UseItemOnItem {
                item: 102,
                item_guid: EntityId(102),
                backpack_slot: 25,
                spell: 5102,
                target_item_guid: EntityId(20),
            }
        );
    }

    #[test]
    fn rogue_poison_restock_lists_queries_and_buys_only_safe_observed_offers() {
        let vendor = EntityId(55);
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(4);
        state.inventory.money = 1_500;
        state.entities.0.insert(
            EntityId(7),
            EntityState {
                id: EntityId(7),
                level: Some(80),
                ..Default::default()
            },
        );
        state.auras.by_entity.entry(EntityId(7)).or_default();
        let now = Instant::now();

        assert_eq!(
            decide_next_with_nearby_vendor(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                now,
                false,
                Some(vendor),
            ),
            MaintenanceDecision::VendorList { vendor }
        );

        state.inventory.vendor = Some(vendor);
        state.inventory.vendor_inventory = Some(wow_state::inventory::VendorInventory {
            vendor,
            offers: vec![wow_state::inventory::VendorOffer {
                slot: 3,
                item: 6947,
                stock: Some(20),
                price_copper: 120,
                buy_count: 5,
                extended_cost: 0,
            }],
        });
        assert_eq!(
            decide_next_with_nearby_vendor(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                now,
                false,
                Some(vendor),
            ),
            MaintenanceDecision::QueryItem { item: 6947 }
        );

        state.inventory.item_metadata.insert(
            6947,
            ItemTemplateMetadata {
                name: "Instant Poison IX".into(),
                item_class: 0,
                subclass: 6,
                allowable_class: 1 << 3,
                required_level: 60,
                ..Default::default()
            },
        );
        assert_eq!(
            decide_next_with_nearby_vendor(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                now,
                false,
                Some(vendor),
            ),
            MaintenanceDecision::VendorBuy {
                vendor,
                item: 6947,
                slot: 3,
                lots: 1,
            }
        );

        let retry = [((u32::MAX, vendor), now + Duration::from_secs(10))]
            .into_iter()
            .collect();
        assert_ne!(
            decide_next_with_nearby_vendor(
                &Snapshot::from_state(&state),
                &retry,
                now,
                false,
                Some(vendor),
            ),
            MaintenanceDecision::VendorBuy {
                vendor,
                item: 6947,
                slot: 3,
                lots: 1,
            }
        );

        state.inventory.money = 1_119;
        assert_ne!(
            decide_next_with_nearby_vendor(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                now,
                false,
                Some(vendor),
            ),
            MaintenanceDecision::VendorBuy {
                vendor,
                item: 6947,
                slot: 3,
                lots: 1,
            }
        );
    }

    #[test]
    fn class_training_queries_nearby_trainer_then_buys_one_eligible_spell() {
        let trainer = EntityId(55);
        let position = wow_domain::WorldPosition {
            map: 1,
            point: wow_domain::Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        };
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(8);
        state.inventory.money = 50_000;
        state.position.player = Some(position);
        state.auras.by_entity.entry(EntityId(7)).or_default();
        state.entities.0.insert(
            EntityId(7),
            EntityState {
                id: EntityId(7),
                level: Some(40),
                position: Some(position),
                ..Default::default()
            },
        );
        state.entities.0.insert(
            trainer,
            EntityState {
                id: trainer,
                kind: wow_state::entities::EntityKind::Unit,
                interactable: true,
                npc_flags: Some(0x30),
                position: Some(position),
                ..Default::default()
            },
        );
        let now = Instant::now();
        let decide = |state: &AuthoritativeState, retry: &BTreeMap<(u32, EntityId), Instant>| {
            decide_next_with_nearby_services(
                &Snapshot::from_state(state),
                retry,
                now,
                false,
                None,
                Some(trainer),
            )
        };
        assert_eq!(
            decide(&state, &BTreeMap::new()),
            MaintenanceDecision::TrainerList { trainer }
        );

        state.trainer = wow_state::trainer::TrainerState {
            trainer: Some(trainer),
            trainer_type: Some(0),
            offers: vec![
                wow_state::trainer::TrainerSpellOffer {
                    spell: 100,
                    usable: 0,
                    cost_copper: 1_000,
                    required_level: 20,
                    required_skill_line: 0,
                    required_skill_rank: 0,
                },
                wow_state::trainer::TrainerSpellOffer {
                    spell: 200,
                    usable: 0,
                    cost_copper: 15_000,
                    required_level: 40,
                    required_skill_line: 0,
                    required_skill_rank: 0,
                },
                wow_state::trainer::TrainerSpellOffer {
                    spell: 300,
                    usable: 1,
                    cost_copper: 1,
                    required_level: 1,
                    required_skill_line: 0,
                    required_skill_rank: 0,
                },
            ],
        };
        assert_eq!(
            decide(&state, &BTreeMap::new()),
            MaintenanceDecision::TrainerBuy {
                trainer,
                spell: 200,
            }
        );
        let purchase_pending = [(u32::MAX, trainer)]
            .into_iter()
            .map(|key| (key, now + Duration::from_secs(10)))
            .collect();
        assert_eq!(
            decide(&state, &purchase_pending),
            MaintenanceDecision::Satisfied
        );

        state.capabilities.spells.insert(200);
        state.inventory.money = 1_999;
        assert_eq!(
            decide(&state, &BTreeMap::new()),
            MaintenanceDecision::Satisfied
        );
        state.inventory.money = 50_000;
        state.trainer.trainer_type = Some(2);
        assert_eq!(
            decide(&state, &BTreeMap::new()),
            MaintenanceDecision::Satisfied
        );
    }
    use wow_state::{
        AuthoritativeState, Snapshot,
        auras::AuraInstance,
        entities::EntityState,
        inventory::{InventoryItemInstance, ItemTemplateMetadata},
    };

    fn authoritative_caster(state: &mut AuthoritativeState, entity: EntityId) {
        state.entities.0.insert(
            entity,
            EntityState {
                id: entity,
                power_type: Some(0),
                power: Some((10_000, 10_000)),
                health: Some((10_000, 10_000)),
                base_health: Some(10_000),
                base_mana: Some(10_000),
                power_cost_modifiers: Some([0; 7]),
                power_cost_multipliers: Some([0.0; 7]),
                shapeshift_form: Some(0),
                aura_state: Some(0),
                ..Default::default()
            },
        );
    }

    fn hunter_state() -> AuthoritativeState {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(3);
        state.capabilities.spells.extend([883, 982]);
        authoritative_caster(&mut state, EntityId(7));
        state.auras.by_entity.entry(EntityId(7)).or_default();
        state.pet.control_known = true;
        state
    }

    fn warlock_state() -> AuthoritativeState {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(9);
        state.capabilities.specialization_tree = Some(0);
        state.capabilities.spells.extend([687, 688, 697]);
        state.inventory.items.insert(6265, 1);
        authoritative_caster(&mut state, EntityId(7));
        state.auras.by_entity.entry(EntityId(7)).or_default();
        state.pet.control_known = true;
        state
    }

    fn warlock_with_active_pet() -> AuthoritativeState {
        let mut state = warlock_state();
        state.pet.guid = Some(EntityId(9));
        state.entities.0.insert(
            EntityId(9),
            EntityState {
                id: EntityId(9),
                health: Some((100, 100)),
                ..Default::default()
            },
        );
        state
    }

    #[test]
    fn pet_care_fails_closed_for_unknown_or_unobserved_pet_state() {
        let mut state = hunter_state();
        state.pet.control_known = false;
        assert!(
            hunter_pet_care(
                &Snapshot::from_state(&state),
                3,
                EntityId(7),
                &BTreeMap::new(),
                Instant::now()
            )
            .is_none()
        );
        state.pet.control_known = true;
        state.pet.guid = Some(EntityId(9));
        assert!(
            hunter_pet_care(
                &Snapshot::from_state(&state),
                3,
                EntityId(7),
                &BTreeMap::new(),
                Instant::now()
            )
            .is_none()
        );
    }

    #[test]
    fn pet_care_calls_a_confirmed_absent_pet_and_revives_a_confirmed_dead_pet() {
        let mut state = hunter_state();
        let now = Instant::now();
        assert!(matches!(
            hunter_pet_care(
                &Snapshot::from_state(&state),
                3,
                EntityId(7),
                &BTreeMap::new(),
                now
            ),
            Some(MaintenanceDecision::Cast { spell: 883, .. })
        ));
        let mut dead = EntityState {
            id: EntityId(9),
            health: Some((0, 100)),
            ..Default::default()
        };
        dead.mark_dead();
        state.pet.guid = Some(EntityId(9));
        state.entities.0.insert(EntityId(9), dead);
        assert!(matches!(
            hunter_pet_care(
                &Snapshot::from_state(&state),
                3,
                EntityId(7),
                &BTreeMap::new(),
                now
            ),
            Some(MaintenanceDecision::Cast { spell: 982, .. })
        ));
        state.entities.0.get_mut(&EntityId(9)).unwrap().health = Some((50, 100));
        assert!(
            hunter_pet_care(
                &Snapshot::from_state(&state),
                3,
                EntityId(7),
                &BTreeMap::new(),
                now
            )
            .is_none()
        );
    }

    #[test]
    fn death_knight_restores_only_the_talented_persistent_ghoul() {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(6);
        state.capabilities.spells.insert(46584);
        state
            .capabilities
            .active_talents
            .push(wow_state::capabilities::TalentRank {
                talent_id: 1984,
                rank: 0,
            });
        authoritative_caster(&mut state, EntityId(7));
        state.auras.by_entity.entry(EntityId(7)).or_default();
        state.pet.control_known = true;
        assert_eq!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false
            ),
            MaintenanceDecision::SummonPet {
                spell: 46584,
                player: EntityId(7)
            }
        );

        state.pet.guid = Some(EntityId(9));
        state.entities.0.insert(
            EntityId(9),
            EntityState {
                id: EntityId(9),
                health: Some((100, 100)),
                ..Default::default()
            },
        );
        assert_ne!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false
            ),
            MaintenanceDecision::SummonPet {
                spell: 46584,
                player: EntityId(7)
            }
        );

        state.capabilities.active_talents.clear();
        state.pet.guid = None;
        assert_ne!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false
            ),
            MaintenanceDecision::SummonPet {
                spell: 46584,
                player: EntityId(7)
            }
        );
    }

    #[test]
    fn mage_restores_water_elemental_only_with_proven_eternal_water_glyph() {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(8);
        state.capabilities.spells.insert(MAGE_WATER_ELEMENTAL_SPELL);
        state.capabilities.active_glyph_properties = Some(vec![871]);
        authoritative_caster(&mut state, EntityId(7));
        state.auras.by_entity.entry(EntityId(7)).or_default();
        state.pet.control_known = true;
        assert_eq!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false
            ),
            MaintenanceDecision::SummonPet {
                spell: MAGE_WATER_ELEMENTAL_SPELL,
                player: EntityId(7)
            }
        );

        state.capabilities.active_glyph_properties = None;
        assert_ne!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false
            ),
            MaintenanceDecision::SummonPet {
                spell: MAGE_WATER_ELEMENTAL_SPELL,
                player: EntityId(7)
            }
        );
        state.capabilities.active_glyph_properties = Some(vec![u16::MAX]);
        assert_ne!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false
            ),
            MaintenanceDecision::SummonPet {
                spell: MAGE_WATER_ELEMENTAL_SPELL,
                player: EntityId(7)
            }
        );
    }

    #[test]
    fn pet_setup_sets_defensive_then_enables_default_autocast() {
        let mut state = hunter_state();
        state.pet.guid = Some(EntityId(9));
        state.pet.reaction = Some(2);
        state.pet.abilities.push(wow_state::pets::PetAbilityState {
            spell: 2641,
            autocast: Some(false),
        });
        let now = Instant::now();
        assert_eq!(
            pet_setup(&Snapshot::from_state(&state), &BTreeMap::new(), now),
            Some(MaintenanceDecision::PetReaction {
                pet: EntityId(9),
                reaction: 1
            })
        );
        state.pet.reaction = Some(1);
        assert_eq!(
            pet_setup(&Snapshot::from_state(&state), &BTreeMap::new(), now),
            Some(MaintenanceDecision::PetAutocast {
                pet: EntityId(9),
                spell: 2641,
                enabled: true
            })
        );
    }

    #[test]
    fn pet_setup_keeps_manual_control_spells_off_and_taunts_solo_only() {
        let mut state = hunter_state();
        state.pet.guid = Some(EntityId(9));
        state.pet.reaction = Some(1);
        state.pet.abilities = vec![
            wow_state::pets::PetAbilityState {
                spell: 19505,
                autocast: Some(true),
            },
            wow_state::pets::PetAbilityState {
                spell: 2649,
                autocast: Some(false),
            },
        ];
        let now = Instant::now();
        assert_eq!(
            pet_setup(&Snapshot::from_state(&state), &BTreeMap::new(), now),
            Some(MaintenanceDecision::PetAutocast {
                pet: EntityId(9),
                spell: 19505,
                enabled: false
            })
        );
        state.pet.abilities.remove(0);
        assert_eq!(
            pet_setup(&Snapshot::from_state(&state), &BTreeMap::new(), now),
            Some(MaintenanceDecision::PetAutocast {
                pet: EntityId(9),
                spell: 2649,
                enabled: true
            })
        );
        state.group.lifecycle = GroupLifecycle::Active;
        assert_eq!(
            pet_setup(&Snapshot::from_state(&state), &BTreeMap::new(), now),
            None
        );
    }

    #[test]
    fn gear_maintenance_selects_a_scored_bag_upgrade() {
        let mut state = hunter_state();
        state.inventory.equipment_slots_authoritative = true;
        state.inventory.instances.insert(
            EntityId(44),
            InventoryItemInstance {
                item: 100,
                guid: EntityId(44),
                backpack_slot: 23,
                count: 1,
            },
        );
        state.inventory.item_metadata.insert(
            100,
            ItemTemplateMetadata {
                item_class: 4,
                inventory_type: 1,
                quality: 2,
                item_level: 10,
                ..Default::default()
            },
        );
        state.inventory.equipped_items.insert(0, 101);
        state
            .entities
            .0
            .get_mut(&EntityId(7))
            .expect("player")
            .level = Some(10);
        assert_eq!(
            gear_upgrade(
                &Snapshot::from_state(&state),
                EntityId(7),
                &BTreeMap::new(),
                Instant::now()
            ),
            None
        );
        state.inventory.item_metadata.insert(
            101,
            ItemTemplateMetadata {
                item_class: 4,
                inventory_type: 1,
                item_level: 1,
                ..Default::default()
            },
        );
        assert_eq!(
            gear_upgrade(
                &Snapshot::from_state(&state),
                EntityId(7),
                &BTreeMap::new(),
                Instant::now()
            ),
            Some(MaintenanceDecision::EquipItem {
                item: 100,
                item_guid: EntityId(44),
                destination_slot: 0,
                player: EntityId(7)
            })
        );
    }

    #[test]
    fn warlock_pet_care_uses_affliction_solo_preference_before_buffs() {
        let state = warlock_state();

        assert_eq!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false
            ),
            MaintenanceDecision::SummonPet {
                spell: 697,
                player: EntityId(7)
            }
        );
    }

    #[test]
    fn warlock_pet_care_uses_group_preference_and_falls_back_when_shard_is_missing() {
        let mut state = warlock_state();
        state.group.lifecycle = GroupLifecycle::Active;
        state.capabilities.spells.insert(691);
        state.inventory.items.clear();
        let decision = decide_next(
            &Snapshot::from_state(&state),
            &BTreeMap::new(),
            Instant::now(),
            false,
        );
        assert!(matches!(
            decision,
            MaintenanceDecision::SummonPet { spell: 688, .. }
        ));
    }

    #[test]
    fn warlock_pet_care_uses_demonology_and_destruction_preferences() {
        let mut state = warlock_state();
        state.capabilities.specialization_tree = Some(1);
        state.capabilities.spells.extend([30146, 691]);
        assert!(matches!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false
            ),
            MaintenanceDecision::SummonPet { spell: 30146, .. }
        ));

        state.capabilities.specialization_tree = Some(2);
        assert!(matches!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false
            ),
            MaintenanceDecision::SummonPet { spell: 688, .. }
        ));
    }

    #[test]
    fn warlock_pet_care_can_probe_unknown_pet_control_and_obeys_retry_window() {
        let mut state = warlock_state();
        state.pet.control_known = false;
        let now = Instant::now();
        let snap = Snapshot::from_state(&state);
        assert!(matches!(
            decide_next(&snap, &BTreeMap::new(), now, false),
            MaintenanceDecision::SummonPet { spell: 697, .. }
        ));
        let retry = BTreeMap::from([((697, EntityId(7)), now + Duration::from_secs(60))]);
        assert!(!matches!(
            decide_next(&snap, &retry, now, false),
            MaintenanceDecision::SummonPet { .. }
        ));
    }

    #[test]
    fn warlock_pet_care_does_not_replace_a_live_pet() {
        let mut state = warlock_state();
        state.pet.guid = Some(EntityId(9));
        state.entities.0.insert(
            EntityId(9),
            EntityState {
                id: EntityId(9),
                health: Some((50, 100)),
                ..Default::default()
            },
        );

        assert!(matches!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false
            ),
            MaintenanceDecision::Cast {
                family,
                spell: 687,
                target: EntityId(7)
            } if family == "warlock_armor"
        ));
    }

    #[test]
    fn warlock_pet_care_uses_a_lower_ready_summon_without_a_soul_shard() {
        let mut state = warlock_state();
        state.inventory.items.clear();

        assert!(matches!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false
            ),
            MaintenanceDecision::SummonPet {
                spell: 688,
                player: EntityId(7)
            }
        ));
    }

    #[test]
    fn mage_chooses_highest_known_intellect_and_stops_when_family_present() {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(8);
        state.capabilities.spells.extend([1459, 1460, 42995]);
        authoritative_caster(&mut state, EntityId(7));
        state.auras.by_entity.entry(EntityId(7)).or_default();
        let snap = Snapshot::from_state(&state);
        let now = Instant::now();
        let retry = BTreeMap::new();
        assert_eq!(
            decide_next(&snap, &retry, now, false),
            MaintenanceDecision::Cast {
                family: "arcane_intellect".into(),
                spell: 42995,
                target: EntityId(7)
            }
        );
        state
            .auras
            .by_entity
            .entry(EntityId(7))
            .or_default()
            .insert(
                1,
                AuraInstance {
                    slot: 1,
                    spell: 42995,
                    positive: Some(true),
                    caster: Some(EntityId(7)),
                    max_duration_ms: None,
                    remaining_ms: None,
                    observed_at_ms: None,
                },
            );
        assert_eq!(
            decide_next(&Snapshot::from_state(&state), &retry, now, false),
            MaintenanceDecision::Satisfied
        );
    }

    #[test]
    fn mage_restocks_drink_before_food_from_authoritative_backpack_state() {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(8);
        state.capabilities.spells.extend([5504, 587]);
        authoritative_caster(&mut state, EntityId(7));
        state.auras.by_entity.entry(EntityId(7)).or_default();
        state.inventory.instances.insert(
            EntityId(70),
            wow_state::inventory::InventoryItemInstance {
                item: 700,
                guid: EntityId(70),
                backpack_slot: 23,
                count: 10,
            },
        );
        state.inventory.item_metadata.insert(
            700,
            wow_state::inventory::ItemTemplateMetadata {
                name: "Conjured Sweet Roll".into(),
                item_class: 0,
                subclass: 5,
                use_spell_id: 1,
                ..Default::default()
            },
        );
        assert_eq!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false
            ),
            MaintenanceDecision::Cast {
                family: "mage_water".into(),
                spell: 5504,
                target: EntityId(7)
            }
        );
        state
            .inventory
            .instances
            .get_mut(&EntityId(70))
            .unwrap()
            .count = 1;
        let now = Instant::now();
        let retry = BTreeMap::from([((5504, EntityId(7)), now + Duration::from_secs(10))]);
        assert_eq!(
            decide_next(&Snapshot::from_state(&state), &retry, now, false),
            MaintenanceDecision::Cast {
                family: "mage_food".into(),
                spell: 587,
                target: EntityId(7)
            }
        );
    }

    #[test]
    fn mage_supply_count_waits_for_backpack_templates_and_counts_refreshments_both_ways() {
        let mut state = AuthoritativeState::default();
        state.inventory.instances.insert(
            EntityId(71),
            wow_state::inventory::InventoryItemInstance {
                item: 701,
                guid: EntityId(71),
                backpack_slot: 23,
                count: 10,
            },
        );
        let snapshot = Snapshot::from_state(&state);
        assert_eq!(mage_food_and_drink(&snapshot), None);
        state.inventory.item_metadata.insert(
            701,
            wow_state::inventory::ItemTemplateMetadata {
                name: "Conjured Refreshment".into(),
                item_class: 0,
                subclass: 5,
                use_spell_id: 1,
                ..Default::default()
            },
        );
        assert_eq!(
            mage_food_and_drink(&Snapshot::from_state(&state)),
            Some((10, 10))
        );
    }

    #[test]
    fn warlock_creates_a_missing_healthstone_before_other_supplies() {
        let mut state = warlock_with_active_pet();
        state.capabilities.spells.insert(6202);
        assert_eq!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false
            ),
            MaintenanceDecision::Cast {
                family: "warlock_create_healthstone".into(),
                spell: 6202,
                target: EntityId(7)
            }
        );
    }

    #[test]
    fn warlock_creates_and_applies_soulstone_through_typed_item_work() {
        let mut state = warlock_with_active_pet();
        state.capabilities.spells.insert(20752);
        assert_eq!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false
            ),
            MaintenanceDecision::Cast {
                family: "warlock_create_soulstone".into(),
                spell: 20752,
                target: EntityId(7)
            }
        );

        state.inventory.instances.insert(
            EntityId(81),
            wow_state::inventory::InventoryItemInstance {
                item: 810,
                guid: EntityId(81),
                backpack_slot: 23,
                count: 1,
            },
        );
        state.inventory.item_metadata.insert(
            810,
            wow_state::inventory::ItemTemplateMetadata {
                name: "Minor Soulstone".into(),
                item_class: 0,
                subclass: 5,
                use_spell_id: 20707,
                ..Default::default()
            },
        );
        assert_eq!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false
            ),
            MaintenanceDecision::UseItemInstance {
                item: 810,
                item_guid: EntityId(81),
                backpack_slot: 23,
                spell: 20707,
                target: EntityId(7)
            }
        );
        state.auras.by_entity.get_mut(&EntityId(7)).unwrap().insert(
            1,
            AuraInstance {
                slot: 1,
                spell: 20707,
                positive: Some(true),
                caster: Some(EntityId(7)),
                max_duration_ms: None,
                remaining_ms: None,
                observed_at_ms: None,
            },
        );
        assert_ne!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false
            ),
            MaintenanceDecision::UseItemInstance {
                item: 810,
                item_guid: EntityId(81),
                backpack_slot: 23,
                spell: 20707,
                target: EntityId(7)
            }
        );
    }
    #[test]
    fn retry_backoff_defers_duplicate_cast_attempt() {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(8);
        state.capabilities.spells.insert(1459);
        state.auras.by_entity.entry(EntityId(7)).or_default();
        let snap = Snapshot::from_state(&state);
        let now = Instant::now();
        let mut retry = BTreeMap::new();
        retry.insert((1459, EntityId(7)), now + Duration::from_secs(10));
        assert_eq!(
            decide_next(&snap, &retry, now, false),
            MaintenanceDecision::Satisfied
        );
    }

    #[test]
    fn controlled_mover_suspends_normal_player_maintenance() {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(8);
        state.capabilities.spells.insert(1459);
        state.auras.by_entity.entry(EntityId(7)).or_default();
        state.control.mover = Some(EntityId(99));
        assert!(matches!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false
            ),
            MaintenanceDecision::Deferred {
                reason: "controlled_mover_active",
                ..
            }
        ));
    }

    #[test]
    fn persistent_pet_retry_classification_covers_all_supported_companions() {
        assert!(is_persistent_pet_summon(688));
        assert!(is_persistent_pet_summon(DEATH_KNIGHT_PERSISTENT_PET));
        assert!(is_persistent_pet_summon(MAGE_WATER_ELEMENTAL_SPELL));
        assert!(!is_persistent_pet_summon(30146 + 1));
    }

    #[test]
    fn low_level_warlock_maintains_demon_skin_before_demon_armor_is_known() {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(9);
        state.capabilities.spells.insert(687);
        authoritative_caster(&mut state, EntityId(7));
        state.auras.by_entity.entry(EntityId(7)).or_default();
        assert_eq!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false
            ),
            MaintenanceDecision::Cast {
                family: "warlock_armor".into(),
                spell: 687,
                target: EntityId(7)
            }
        );
    }

    #[test]
    fn party_member_out_of_range_or_already_satisfied_is_skipped() {
        use wow_domain::{Vec3, WorldPosition};
        use wow_state::{
            entities::{EntityKind, EntityState},
            group::{GroupMember, GroupState},
        };
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(8);
        state.capabilities.spells.insert(1459);
        state
            .auras
            .by_entity
            .entry(EntityId(7))
            .or_default()
            .insert(
                1,
                AuraInstance {
                    slot: 1,
                    spell: 1459,
                    positive: Some(true),
                    caster: Some(EntityId(7)),
                    max_duration_ms: None,
                    remaining_ms: None,
                    observed_at_ms: None,
                },
            );
        state.entities.0.insert(
            EntityId(7),
            EntityState {
                id: EntityId(7),
                kind: EntityKind::Player,
                position: Some(WorldPosition {
                    map: 0,
                    point: Vec3::new(0.0, 0.0, 0.0),
                    orientation: 0.0,
                }),
                ..Default::default()
            },
        );
        state.entities.0.insert(
            EntityId(8),
            EntityState {
                id: EntityId(8),
                kind: EntityKind::Player,
                position: Some(WorldPosition {
                    map: 0,
                    point: Vec3::new(100.0, 0.0, 0.0),
                    orientation: 0.0,
                }),
                ..Default::default()
            },
        );
        state.auras.by_entity.entry(EntityId(8)).or_default();
        state.group = GroupState {
            members: vec![GroupMember {
                entity: EntityId(8),
                online: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                true
            ),
            MaintenanceDecision::Satisfied
        );
    }
}
