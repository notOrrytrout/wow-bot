use super::packet::{read_packed_guid, read_u32_cursor as read_u32};
use wow_domain::{
    EntityId, Vec3, WorldPosition,
    binary::{cstring_at, u32_le_at, u64_le_at},
    time::Millis,
};
use wow_state::ProtocolObservation;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battleground_world_state_packets_decode_full_snapshots_and_updates() {
        let mut packet = Vec::new();
        packet.extend_from_slice(&30_i32.to_le_bytes());
        packet.extend_from_slice(&1377_i32.to_le_bytes());
        packet.extend_from_slice(&0_i32.to_le_bytes());
        packet.extend_from_slice(&2_u16.to_le_bytes());
        for (variable, value) in [(4247_i32, 1_i32), (1601, -1)] {
            packet.extend_from_slice(&variable.to_le_bytes());
            packet.extend_from_slice(&value.to_le_bytes());
        }
        assert!(matches!(
            maintenance_observations(0x02C2, &packet).as_slice(),
            [ProtocolObservation::BattlegroundWorldStatesInitialized {
                map_id: 30, zone_id: 1377, area_id: 0, states,
            }] if states == &vec![(4247, 1), (1601, -1)]
        ));
        assert!(maintenance_observations(0x02C2, &packet[..packet.len() - 1]).is_empty());

        let mut update = 4247_i32.to_le_bytes().to_vec();
        update.extend_from_slice(&0_i32.to_le_bytes());
        assert!(matches!(
            maintenance_observations(0x02C3, &update).as_slice(),
            [ProtocolObservation::BattlegroundWorldStateUpdated {
                variable: 4247,
                value: 0
            }]
        ));
        assert!(maintenance_observations(0x02C3, &update[..7]).is_empty());
    }

    #[test]
    fn show_bank_observation_keeps_the_authoritative_banker_guid() {
        let banker = 0x1122_3344_5566_7788_u64;
        let body = banker.to_le_bytes();
        assert!(matches!(
            maintenance_observations(0x01B8, &body).as_slice(),
            [ProtocolObservation::BankOpened { banker: EntityId(guid) }] if *guid == banker
        ));
        assert!(maintenance_observations(0x01B8, &body[..7]).is_empty());
        assert!(maintenance_observations(0x01B8, &[0; 8]).is_empty());
    }

    #[test]
    fn item_template_observation_keeps_fields_needed_for_gear_scoring() {
        let mut packet = Vec::new();
        for value in [100, 4, 1, 0] {
            packet.extend_from_slice(&value_u32(value));
        }
        packet.extend_from_slice(b"Conjured Frostwater\0");
        for _ in 0..3 {
            packet.extend_from_slice(b"\0");
        }
        for value in [1, 2, 0, 0, 0, 10, 10, 0, 0, 70, 10] {
            packet.extend_from_slice(&value_u32(value));
        }
        for value in [0; 9] {
            packet.extend_from_slice(&value_u32(value));
        }
        packet.extend_from_slice(&value_u32(0)); // container slots
        packet.extend_from_slice(&value_u32(1)); // one stat
        packet.extend_from_slice(&value_u32(4));
        packet.extend_from_slice(&value_u32(12));
        packet.extend_from_slice(&[0; 8]); // scaling
        for value in [1.0_f32, 2.0] {
            packet.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        packet.extend_from_slice(&value_u32(0));
        packet.extend_from_slice(&[0; 8]); // second damage range
        packet.extend_from_slice(&value_u32(0));
        packet.extend_from_slice(&value_u32(25)); // armor
        packet.extend_from_slice(&[0; 24]);
        packet.extend_from_slice(&value_u32(1800));
        packet.extend_from_slice(&value_u32(1));
        packet.extend_from_slice(&value_u32(0)); // ranged modifier
        packet.extend_from_slice(&value_u32(1234)); // item use spell
        packet.extend_from_slice(&value_u32(0)); // on use
        packet.extend_from_slice(&[0; 16]);
        for _ in 0..4 {
            packet.extend_from_slice(&[0; 24]);
        }

        let Some(ProtocolObservation::ItemTemplate { item, metadata }) =
            parse_item_template(&packet)
        else {
            panic!("item metadata expected")
        };
        assert_eq!(item, 100);
        assert_eq!(metadata.inventory_type, 10);
        assert_eq!(metadata.item_level, 70);
        assert_eq!(metadata.required_level, 10);
        assert_eq!(metadata.stats, vec![(4, 12)]);
        assert_eq!(metadata.armor, 25);
        assert_eq!(metadata.delay_ms, 1800);
        assert_eq!(metadata.name, "Conjured Frostwater");
        assert_eq!(metadata.use_spell_id, 1234);
    }

    #[test]
    fn mail_list_parser_preserves_cod_money_and_attachment_identity() {
        let mut mail = Vec::new();
        mail.extend_from_slice(&77_u32.to_le_bytes());
        mail.push(0); // normal mail
        mail.extend_from_slice(&0x1122_3344_5566_7788_u64.to_le_bytes());
        mail.extend_from_slice(&0_u32.to_le_bytes()); // COD
        mail.extend_from_slice(&0_u32.to_le_bytes()); // compatibility field
        mail.extend_from_slice(&41_u32.to_le_bytes()); // stationery
        mail.extend_from_slice(&12_345_u32.to_le_bytes()); // attached money
        mail.extend_from_slice(&0_u32.to_le_bytes()); // checked flags
        mail.extend_from_slice(&1.0_f32.to_le_bytes()); // expiry
        mail.extend_from_slice(&0_u32.to_le_bytes()); // template
        mail.extend_from_slice(b"subject\0body\0");
        mail.push(1); // attachment count
        mail.push(0); // attachment slot
        mail.extend_from_slice(&9001_u32.to_le_bytes()); // low GUID
        mail.extend_from_slice(&4306_u32.to_le_bytes()); // item ID
        mail.extend_from_slice(&[0; 84]); // enchantments
        mail.extend_from_slice(&0_i32.to_le_bytes()); // random property
        mail.extend_from_slice(&0_u32.to_le_bytes()); // suffix factor
        mail.push(4); // stack count
        mail.extend_from_slice(&0_u32.to_le_bytes()); // spell charges
        mail.extend_from_slice(&0_u32.to_le_bytes()); // max durability
        mail.extend_from_slice(&0_u32.to_le_bytes()); // durability
        mail.push(0); // WotLK unknown

        let mut body = Vec::new();
        body.extend_from_slice(&1_u32.to_le_bytes()); // server total
        body.push(1); // visible count
        body.extend_from_slice(&(mail.len() as u16).to_le_bytes());
        body.extend_from_slice(&mail);
        let Some(ProtocolObservation::Mailbox(mailbox)) = parse_mail_list(&body) else {
            panic!("expected authoritative mailbox observation");
        };
        assert!(mailbox.authoritative);
        let mail = mailbox.mails.get(&77).expect("mail entry");
        assert_eq!(mail.cod_copper, Some(0));
        assert_eq!(mail.money, 12_345);
        assert_eq!(mail.attachments.get(&9001).unwrap().item_id, 4306);
        assert_eq!(mail.attachments.get(&9001).unwrap().count, 4);
        assert!(matches!(
            maintenance_observations(0x023B, &body).as_slice(),
            [ProtocolObservation::Mailbox(_)]
        ));

        let mut cod_mail = body.clone();
        let cod_offset = 5 + 2 + 4 + 1 + 8;
        cod_mail[cod_offset..cod_offset + 4].copy_from_slice(&100_u32.to_le_bytes());
        let Some(ProtocolObservation::Mailbox(cod_box)) = parse_mail_list(&cod_mail) else {
            panic!("nonzero COD must remain represented");
        };
        assert_eq!(cod_box.mails[&77].cod_copper, Some(100));
        assert!(parse_mail_list(&body[..body.len() - 1]).is_none());
    }

    #[test]
    fn vendor_list_and_buy_packets_preserve_observed_offer_fields() {
        let vendor = 0x1122_3344_5566_7788_u64;
        let mut packet = vendor.to_le_bytes().to_vec();
        packet.push(2);
        for (slot, item, stock, price, buy_count, extended_cost) in [
            (1_u32, 6947_u32, -1_i32, 120_u32, 5_u32, 0_u32),
            (2, 2892, 7, 200, 5, 0),
        ] {
            for value in [slot, item, 0] {
                packet.extend_from_slice(&value.to_le_bytes());
            }
            packet.extend_from_slice(&stock.to_le_bytes());
            for value in [price, 0, buy_count, extended_cost] {
                packet.extend_from_slice(&value.to_le_bytes());
            }
        }
        let ProtocolObservation::VendorInventory {
            vendor: observed,
            offers,
        } = parse_vendor_list(&packet).expect("valid vendor list")
        else {
            panic!("vendor inventory observation expected")
        };
        assert_eq!(observed, EntityId(vendor));
        assert_eq!(offers.len(), 2);
        assert_eq!(offers[0].stock, None);
        assert_eq!(offers[0].buy_count, 5);
        assert_eq!(offers[1].stock, Some(7));
        assert_eq!(offers[1].price_copper, 200);
        assert!(parse_vendor_list(&packet[..packet.len() - 1]).is_none());

        let mut bought = vendor.to_le_bytes().to_vec();
        bought.extend_from_slice(&2_u32.to_le_bytes());
        bought.extend_from_slice(&2_i32.to_le_bytes());
        bought.extend_from_slice(&1_u32.to_le_bytes());
        assert!(matches!(
            parse_vendor_buy_response(&bought),
            Some(ProtocolObservation::VendorStockUpdated {
                vendor: EntityId(guid), slot: 2, stock: Some(2), purchased_lots: 1,
            }) if guid == vendor
        ));
    }

    #[test]
    fn trainer_list_keeps_authoritative_spell_cost_and_requirements() {
        let trainer = 0x1122_3344_5566_7788_u64;
        let mut packet = trainer.to_le_bytes().to_vec();
        packet.extend_from_slice(&0_i32.to_le_bytes()); // class trainer
        packet.extend_from_slice(&1_u32.to_le_bytes());
        packet.extend_from_slice(&1234_u32.to_le_bytes());
        packet.push(0); // available
        packet.extend_from_slice(&25_000_u32.to_le_bytes());
        packet.extend_from_slice(&[0; 8]); // point costs
        packet.push(20); // required level
        packet.extend_from_slice(&164_u32.to_le_bytes()); // skill line
        packet.extend_from_slice(&75_u32.to_le_bytes()); // skill rank
        packet.extend_from_slice(&[0; 12]); // prerequisite spells
        packet.extend_from_slice(b"Trainer\0");

        let Some(ProtocolObservation::TrainerList {
            trainer: observed,
            trainer_type,
            offers,
        }) = parse_trainer_list(&packet)
        else {
            panic!("trainer list observation expected")
        };
        assert_eq!(observed, EntityId(trainer));
        assert_eq!(trainer_type, 0);
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].spell, 1234);
        assert_eq!(offers[0].cost_copper, 25_000);
        assert_eq!(offers[0].required_level, 20);
        assert_eq!(offers[0].required_skill_line, 164);
        assert_eq!(offers[0].required_skill_rank, 75);
        assert!(matches!(
            maintenance_observations(0x01b1, &packet).as_slice(),
            [ProtocolObservation::TrainerList { trainer: found, .. }] if *found == EntityId(trainer)
        ));
        assert!(parse_trainer_list(&packet[..packet.len() - 9]).is_none());
    }

    fn value_u32(value: u32) -> [u8; 4] {
        value.to_le_bytes()
    }

    #[test]
    fn rune_observation_requires_six_valid_runes() {
        let mut packet = 6u32.to_le_bytes().to_vec();
        packet.extend_from_slice(&[0, 255, 1, 0, 2, 255, 3, 0, 1, 255, 2, 0]);
        let runes = parse_player_runes(&packet).expect("six rune states");
        assert_eq!(runes.len(), 6);
        assert!(runes[0].ready);
        assert!(!runes[1].ready);

        assert!(parse_player_runes(&packet[..packet.len() - 1]).is_none());
        packet[4] = 4;
        assert!(parse_player_runes(&packet).is_none());
    }

    #[test]
    fn combo_points_decode_packed_target_and_reject_trailing_bytes() {
        let packet = [0b0000_0010, 0x34, 5];
        assert!(matches!(
            parse_combo_points(&packet),
            ProtocolObservation::ComboPoints {
                target: Some(EntityId(0x3400)),
                points: Some(5),
            }
        ));
        assert!(matches!(
            parse_combo_points(&[0b0000_0010, 0x34, 5, 9]),
            ProtocolObservation::ComboPoints {
                target: Some(EntityId(0x3400)),
                points: None,
            }
        ));
    }

    #[test]
    fn cooldown_flags_observe_global_cooldown_start() {
        let mut packet = vec![0; 9];
        packet[8] = 1;
        packet.extend_from_slice(&123u32.to_le_bytes());
        packet.extend_from_slice(&500u32.to_le_bytes());
        let observations = parse_spell_cooldowns(&packet);
        assert_eq!(observations.len(), 2);
        assert!(matches!(
            observations[0],
            ProtocolObservation::SpellCooldown { spell: 123, .. }
        ));
        assert!(matches!(
            observations[1],
            ProtocolObservation::SpellGlobalCooldown { spell: 123, .. }
        ));
    }

    #[test]
    fn cast_start_and_channel_packets_share_millisecond_deadlines() {
        let spell = 133u32;
        let duration = 1_500u32;

        let mut spell_start = vec![0, 1, 0x34, 1];
        spell_start.extend_from_slice(&spell.to_le_bytes());
        spell_start.extend_from_slice(&0u32.to_le_bytes());
        spell_start.extend_from_slice(&duration.to_le_bytes());
        let ProtocolObservation::CastStarted {
            caster,
            spell: observed_spell,
            started_at_ms,
            ends_at_ms,
        } = parse_spell_start(&spell_start).expect("valid spell start")
        else {
            panic!("spell start observation expected")
        };
        assert_eq!(caster, EntityId(0x34));
        assert_eq!(observed_spell, spell);
        assert_eq!(ends_at_ms - started_at_ms, u64::from(duration));
        assert!(parse_spell_start(&spell_start[..spell_start.len() - 1]).is_none());

        let mut channel_start = vec![1, 0x34];
        channel_start.extend_from_slice(&spell.to_le_bytes());
        channel_start.extend_from_slice(&duration.to_le_bytes());
        let ProtocolObservation::CastStarted {
            started_at_ms,
            ends_at_ms,
            ..
        } = parse_channel_start(&channel_start).expect("valid channel start")
        else {
            panic!("channel start observation expected")
        };
        assert_eq!(ends_at_ms - started_at_ms, u64::from(duration));

        let mut channel_update = vec![1, 0x34];
        channel_update.extend_from_slice(&duration.to_le_bytes());
        assert!(matches!(
            parse_channel_update(&channel_update),
            Some(ProtocolObservation::CastUpdated { caster: EntityId(0x34), ends_at_ms })
                if ends_at_ms > 0
        ));
        channel_update[2..].copy_from_slice(&0u32.to_le_bytes());
        assert!(matches!(
            parse_channel_update(&channel_update),
            Some(ProtocolObservation::CastUpdated { ends_at_ms: 0, .. })
        ));
    }

    #[test]
    fn single_and_multiple_quest_statuses_share_the_record_layout() {
        let mut entry = 77_u64.to_le_bytes().to_vec();
        entry.push(3);
        assert!(matches!(
            parse_single_quest_status(&entry),
            Some(ProtocolObservation::QuestGiverStatus {
                giver: EntityId(77),
                status: 3
            })
        ));
        let mut multiple = 1_u32.to_le_bytes().to_vec();
        multiple.extend_from_slice(&entry);
        assert!(matches!(
            parse_multiple_quest_status(&multiple).as_slice(),
            [ProtocolObservation::QuestGiverStatus {
                giver: EntityId(77),
                status: 3
            }]
        ));
        assert!(parse_single_quest_status(&entry[..8]).is_none());
    }

    #[test]
    fn questgiver_complete_packet_does_not_confirm_quest_journal_completion() {
        const SMSG_QUESTGIVER_QUEST_COMPLETE: u32 = 0x018F;
        let observations =
            quest_observations(SMSG_QUESTGIVER_QUEST_COMPLETE, &170_u32.to_le_bytes());

        assert!(observations.is_empty());
    }

    #[test]
    fn quest_list_and_gossip_decode_the_same_offer_record() {
        let mut record = 77_u32.to_le_bytes().to_vec();
        record.extend_from_slice(&9_u32.to_le_bytes());
        record.extend_from_slice(&[0; 9]);
        record.extend_from_slice(b"A quest\0");

        let mut quest_list = 3_u64.to_le_bytes().to_vec();
        quest_list.extend_from_slice(b"NPC\0");
        quest_list.extend_from_slice(&[0; 8]);
        quest_list.push(1);
        quest_list.extend_from_slice(&record);
        assert!(matches!(
            parse_quest_list(&quest_list).as_slice(),
            [
                ProtocolObservation::QuestGiverListReceived { offer_count: 1, .. },
                ProtocolObservation::QuestOffer {
                    quest: 77,
                    icon: 9,
                    ..
                }
            ]
        ));

        let mut gossip = 3_u64.to_le_bytes().to_vec();
        gossip.extend_from_slice(&[0; 8]);
        gossip.extend_from_slice(&0_u32.to_le_bytes());
        gossip.extend_from_slice(&1_u32.to_le_bytes());
        gossip.extend_from_slice(&record);
        assert!(matches!(
            parse_gossip_message(&gossip).as_slice(),
            [
                ProtocolObservation::QuestGiverListReceived { offer_count: 1, .. },
                ProtocolObservation::QuestOffer {
                    quest: 77,
                    icon: 9,
                    ..
                }
            ]
        ));

        let mut offset = 0;
        assert_eq!(
            parse_quest_offer_record(&record, &mut offset),
            Some((77, 9))
        );
        assert_eq!(offset, record.len());
        assert!(parse_quest_offer_record(&record[..record.len() - 1], &mut 0).is_none());

        record[..4].copy_from_slice(&0_u32.to_le_bytes());
        let mut gossip_zero = 3_u64.to_le_bytes().to_vec();
        gossip_zero.extend_from_slice(&[0; 8]);
        gossip_zero.extend_from_slice(&0_u32.to_le_bytes());
        gossip_zero.extend_from_slice(&1_u32.to_le_bytes());
        gossip_zero.extend_from_slice(&record);
        assert!(matches!(
            parse_gossip_message(&gossip_zero).as_slice(),
            [ProtocolObservation::QuestGiverListReceived { offer_count: 1, .. }]
        ));
    }
}

pub(super) fn maintenance_observations(opcode: u32, body: &[u8]) -> Vec<ProtocolObservation> {
    const SMSG_INIT_WORLD_STATES: u32 = 0x02C2;
    const SMSG_UPDATE_WORLD_STATE: u32 = 0x02C3;
    const SMSG_INITIAL_SPELLS: u32 = 0x012A;
    const SMSG_LEARNED_SPELL: u32 = 0x012B;
    const SMSG_AURA_UPDATE_ALL: u32 = 0x0495;
    const SMSG_AURA_UPDATE: u32 = 0x0496;
    const SMSG_SPELL_COOLDOWN: u32 = 0x0134;
    const SMSG_SPELL_START: u32 = 0x0131;
    const SMSG_SPELL_GO: u32 = 0x0132;
    const MSG_CHANNEL_START: u32 = 0x0139;
    const MSG_CHANNEL_UPDATE: u32 = 0x013A;
    const SMSG_RESYNC_RUNES: u32 = 0x0487;
    const SMSG_UPDATE_COMBO_POINTS: u32 = 0x039D;
    const MSG_CORPSE_QUERY: u32 = 0x0216;
    const SMSG_CORPSE_RECLAIM_DELAY: u32 = 0x0269;
    const SMSG_PARTYKILLLOG: u32 = 0x01F5;
    const SMSG_ITEM_QUERY_SINGLE_RESPONSE: u32 = 0x0058;
    const SMSG_LIST_INVENTORY: u32 = 0x019F;
    const SMSG_BUY_ITEM: u32 = 0x01A4;
    const SMSG_TRAINER_LIST: u32 = 0x01B1;
    const SMSG_MAIL_LIST_RESULT: u32 = 0x023B;
    const SMSG_SHOW_BANK: u32 = 0x01B8;
    const SMSG_GROUP_UNINVITE: u32 = 0x0077;
    const SMSG_GROUP_DESTROYED: u32 = 0x007C;
    const SMSG_GROUP_LIST: u32 = 0x007D;
    const SMSG_LOOT_START_ROLL: u32 = 0x02A1;
    match opcode {
        SMSG_INIT_WORLD_STATES => parse_initial_world_states(body).into_iter().collect(),
        SMSG_UPDATE_WORLD_STATE => parse_world_state_update(body).into_iter().collect(),
        SMSG_GROUP_LIST => parse_group_loot_method(body).into_iter().collect(),
        SMSG_GROUP_UNINVITE | SMSG_GROUP_DESTROYED => {
            vec![ProtocolObservation::GroupLootMethod(None)]
        }
        SMSG_LOOT_START_ROLL => parse_group_loot_roll(body).into_iter().collect(),
        SMSG_ITEM_QUERY_SINGLE_RESPONSE => parse_item_template(body).into_iter().collect(),
        SMSG_LIST_INVENTORY => parse_vendor_list(body).into_iter().collect(),
        SMSG_BUY_ITEM => parse_vendor_buy_response(body).into_iter().collect(),
        SMSG_TRAINER_LIST => parse_trainer_list(body).into_iter().collect(),
        SMSG_MAIL_LIST_RESULT => parse_mail_list(body).into_iter().collect(),
        SMSG_SHOW_BANK => parse_bank_opened(body).into_iter().collect(),
        SMSG_INITIAL_SPELLS => parse_initial_spells(body),
        SMSG_LEARNED_SPELL => body
            .get(0..4)
            .map(|bytes| {
                vec![ProtocolObservation::SpellKnown {
                    spell: u32_le_at(bytes, 0).unwrap_or_default(),
                }]
            })
            .unwrap_or_default(),
        SMSG_AURA_UPDATE_ALL => parse_aura_update_all(body).into_iter().collect(),
        SMSG_AURA_UPDATE => parse_aura_update(body).into_iter().collect(),
        SMSG_SPELL_COOLDOWN => parse_spell_cooldowns(body),
        SMSG_SPELL_START => parse_spell_start(body).into_iter().collect(),
        SMSG_SPELL_GO => parse_spell_go(body).into_iter().collect(),
        MSG_CHANNEL_START => parse_channel_start(body).into_iter().collect(),
        MSG_CHANNEL_UPDATE => parse_channel_update(body).into_iter().collect(),
        SMSG_RESYNC_RUNES => vec![ProtocolObservation::PlayerRunes {
            runes: parse_player_runes(body),
        }],
        SMSG_UPDATE_COMBO_POINTS => vec![parse_combo_points(body)],
        MSG_CORPSE_QUERY => parse_corpse_query(body).into_iter().collect(),
        SMSG_CORPSE_RECLAIM_DELAY => parse_reclaim_delay(body).into_iter().collect(),
        SMSG_PARTYKILLLOG => parse_creature_killed(body).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn parse_initial_world_states(body: &[u8]) -> Option<ProtocolObservation> {
    let map_id = i32::from_le_bytes(body.get(0..4)?.try_into().ok()?) as u32;
    let zone_id = i32::from_le_bytes(body.get(4..8)?.try_into().ok()?) as u32;
    let area_id = i32::from_le_bytes(body.get(8..12)?.try_into().ok()?) as u32;
    let count = u16::from_le_bytes(body.get(12..14)?.try_into().ok()?) as usize;
    if count > 4096 || body.len() < 14 + count * 8 {
        return None;
    }
    let states = (0..count)
        .map(|index| {
            let offset = 14 + index * 8;
            let variable =
                i32::from_le_bytes(body.get(offset..offset + 4)?.try_into().ok()?) as u32;
            let value = i32::from_le_bytes(body.get(offset + 4..offset + 8)?.try_into().ok()?);
            Some((variable, value))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(ProtocolObservation::BattlegroundWorldStatesInitialized {
        map_id,
        zone_id,
        area_id,
        states,
    })
}

fn parse_world_state_update(body: &[u8]) -> Option<ProtocolObservation> {
    Some(ProtocolObservation::BattlegroundWorldStateUpdated {
        variable: i32::from_le_bytes(body.get(0..4)?.try_into().ok()?) as u32,
        value: i32::from_le_bytes(body.get(4..8)?.try_into().ok()?),
    })
}

fn parse_group_loot_method(body: &[u8]) -> Option<ProtocolObservation> {
    let group_type = *body.first()?;
    let mut cursor = 4usize;
    if group_type & 0x08 != 0 {
        cursor = cursor.checked_add(5)?;
    }
    cursor = cursor.checked_add(8 + 4)?; // group GUID and update counter
    let count = usize::try_from(u32_le_at(body, cursor)?).ok()?;
    cursor = cursor.checked_add(4)?;
    if count > 39 {
        return None;
    }
    for _ in 0..count {
        let name_length = body.get(cursor..)?.iter().position(|byte| *byte == 0)?;
        cursor = cursor.checked_add(name_length + 1 + 8 + 4)?;
        body.get(..cursor)?;
    }
    let leader = u64_le_at(body, cursor)?;
    cursor = cursor.checked_add(8)?;
    let method = if count > 0 && leader != 0 {
        Some(wow_state::group::GroupLootMethod::from_wire(
            *body.get(cursor)?,
        ))
    } else {
        None
    };
    Some(ProtocolObservation::GroupLootMethod(method))
}

fn parse_group_loot_roll(body: &[u8]) -> Option<ProtocolObservation> {
    let item_guid = u64_le_at(body, 0)?;
    let map_id = u32_le_at(body, 8)?;
    let item_slot = u32_le_at(body, 12)?;
    let item_id = u32_le_at(body, 16)?;
    // Random suffix and property fields are protocol facts that policy does not use.
    let item_count = u32_le_at(body, 28)?;
    let countdown_ms = u32_le_at(body, 32)?;
    let vote_mask = *body.get(36)?;
    if item_guid == 0 || item_id == 0 || vote_mask == 0 {
        return None;
    }
    Some(ProtocolObservation::GroupLootRollStarted(
        wow_state::group::GroupLootRollRequest {
            item: EntityId(item_guid),
            map_id,
            item_slot,
            item_id,
            item_count,
            countdown_ms,
            vote_mask,
        },
    ))
}

#[cfg(test)]
mod group_loot_observation_tests {
    use super::*;

    fn group_list(method: u8) -> Vec<u8> {
        let mut body = vec![0; 4]; // party flags, subgroup, flags, roles
        body.extend_from_slice(&[0; 8]); // group GUID
        body.extend_from_slice(&0_u32.to_le_bytes()); // update counter
        body.extend_from_slice(&1_u32.to_le_bytes()); // one other member
        body.extend_from_slice(b"Member\0");
        body.extend_from_slice(&7_u64.to_le_bytes()); // member GUID
        body.extend_from_slice(&[1, 0, 0, 0]); // online, subgroup, flags, roles
        body.extend_from_slice(&8_u64.to_le_bytes()); // leader GUID
        body.push(method);
        body.extend_from_slice(&8_u64.to_le_bytes()); // master looter GUID
        body.push(2); // threshold
        body
    }

    #[test]
    fn group_list_projects_known_and_unknown_server_loot_methods() {
        assert!(matches!(
            maintenance_observations(0x007D, &group_list(4)).as_slice(),
            [ProtocolObservation::GroupLootMethod(Some(
                wow_state::group::GroupLootMethod::NeedBeforeGreed
            ))]
        ));
        assert!(matches!(
            maintenance_observations(0x007D, &group_list(9)).as_slice(),
            [ProtocolObservation::GroupLootMethod(Some(
                wow_state::group::GroupLootMethod::Unknown(9)
            ))]
        ));
        assert!(maintenance_observations(0x007D, &group_list(4)[..20]).is_empty());
        assert!(matches!(
            maintenance_observations(0x007C, &[]).as_slice(),
            [ProtocolObservation::GroupLootMethod(None)]
        ));
    }

    #[test]
    fn loot_roll_start_keeps_server_item_identity_and_allowed_vote_mask() {
        let mut body = Vec::new();
        body.extend_from_slice(&0x1122_u64.to_le_bytes());
        body.extend_from_slice(&571_u32.to_le_bytes());
        body.extend_from_slice(&3_u32.to_le_bytes());
        body.extend_from_slice(&1234_u32.to_le_bytes());
        body.extend_from_slice(&0_u32.to_le_bytes()); // random suffix
        body.extend_from_slice(&0_u32.to_le_bytes()); // random property
        body.extend_from_slice(&2_u32.to_le_bytes());
        body.extend_from_slice(&30_000_u32.to_le_bytes());
        body.push(0b0111);
        let observations = maintenance_observations(0x02A1, &body);
        let [ProtocolObservation::GroupLootRollStarted(request)] = observations.as_slice() else {
            panic!("valid server roll request expected");
        };
        assert_eq!(request.item, EntityId(0x1122));
        assert_eq!(request.map_id, 571);
        assert_eq!(request.item_slot, 3);
        assert_eq!(request.item_id, 1234);
        assert_eq!(request.item_count, 2);
        assert_eq!(request.countdown_ms, 30_000);
        assert!(request.allows(0));
        assert!(request.allows(2));
        assert!(!request.allows(3));
        assert!(maintenance_observations(0x02A1, &body[..36]).is_empty());
        body[0..8].fill(0);
        assert!(maintenance_observations(0x02A1, &body).is_empty());
    }
}

fn parse_bank_opened(body: &[u8]) -> Option<ProtocolObservation> {
    if body.len() != 8 {
        return None;
    }
    let banker = EntityId(u64_le_at(body, 0)?);
    (banker.0 != 0).then_some(ProtocolObservation::BankOpened { banker })
}

fn parse_mail_list(body: &[u8]) -> Option<ProtocolObservation> {
    const MAX_MAILS: usize = 50;
    const MAX_ATTACHMENTS: usize = 12;
    const ATTACHMENT_ENCHANT_BYTES: usize = 7 * 3 * 4;
    let mut header_cursor = 0;
    let total = read_u32(body, &mut header_cursor)?;
    let count = usize::from(*body.get(4)?);
    if count > MAX_MAILS || total > 10_000 {
        return None;
    }
    let mut cursor = 5usize;
    let mut mails = std::collections::BTreeMap::new();
    for _ in 0..count {
        let size = usize::from(u16::from_le_bytes(
            body.get(cursor..cursor.checked_add(2)?)?.try_into().ok()?,
        ));
        cursor = cursor.checked_add(2)?;
        if size == 0 || size > body.len().saturating_sub(cursor) {
            return None;
        }
        let end = cursor.checked_add(size)?;
        let mail = body.get(cursor..end)?;
        cursor = end;
        let mut offset = 0usize;
        let mail_id = read_u32(mail, &mut offset)?;
        let message_type = *mail.get(offset)?;
        offset += 1;
        match message_type {
            0 => offset = offset.checked_add(8)?,
            2..=5 => offset = offset.checked_add(4)?,
            _ => return None,
        }
        let cod = read_u32(mail, &mut offset)?;
        offset = offset.checked_add(4 + 4)?; // compatibility field and stationery
        let money = read_u32(mail, &mut offset)?;
        offset = offset.checked_add(4 + 4 + 4)?; // checked flags, expiry, template
        for _ in 0..2 {
            let terminator = mail.get(offset..)?.iter().position(|byte| *byte == 0)?;
            offset = offset.checked_add(terminator + 1)?;
        }
        let attachment_count = usize::from(*mail.get(offset)?);
        offset += 1;
        if attachment_count > MAX_ATTACHMENTS {
            return None;
        }
        let mut attachments = std::collections::BTreeMap::new();
        for _ in 0..attachment_count {
            offset = offset.checked_add(1)?; // attachment slot
            let low_guid = read_u32(mail, &mut offset)?;
            let item_id = read_u32(mail, &mut offset)?;
            offset = offset.checked_add(ATTACHMENT_ENCHANT_BYTES + 4 + 4)?;
            let item_count = u32::from(*mail.get(offset)?);
            offset += 1;
            offset = offset.checked_add(4 + 4 + 4 + 1)?;
            if low_guid != 0 && item_id != 0 && item_count != 0 {
                attachments.insert(
                    low_guid,
                    wow_state::inventory::MailAttachment {
                        item_id,
                        count: item_count,
                    },
                );
            }
        }
        if offset > mail.len() || mail_id == 0 {
            return None;
        }
        mails.insert(
            mail_id,
            wow_state::inventory::MailEntry {
                mail_id,
                money: u64::from(money),
                cod_copper: Some(u64::from(cod)),
                attachments,
            },
        );
    }
    Some(ProtocolObservation::Mailbox(
        wow_state::inventory::MailboxState {
            generation: 0,
            authoritative: true,
            mails,
        },
    ))
}

fn parse_trainer_list(body: &[u8]) -> Option<ProtocolObservation> {
    const MAX_TRAINER_OFFERS: usize = 1024;
    let trainer = EntityId(u64_le_at(body, 0)?);
    if trainer.0 == 0 {
        return None;
    }
    let trainer_type = u32_le_at(body, 8)? as i32;
    let count = usize::try_from(u32_le_at(body, 12)?).ok()?;
    if count > MAX_TRAINER_OFFERS {
        return None;
    }
    let mut cursor = 16;
    let mut offers = Vec::with_capacity(count);
    for _ in 0..count {
        let spell = read_u32(body, &mut cursor)?;
        let usable = *body.get(cursor)?;
        cursor += 1;
        let cost_copper = read_u32(body, &mut cursor)?;
        cursor = cursor.checked_add(8)?; // talent and profession point costs
        let required_level = *body.get(cursor)?;
        cursor += 1;
        let required_skill_line = read_u32(body, &mut cursor)?;
        let required_skill_rank = read_u32(body, &mut cursor)?;
        cursor = cursor.checked_add(12)?; // three prerequisite abilities
        if cursor > body.len() || spell == 0 {
            return None;
        }
        offers.push(wow_state::trainer::TrainerSpellOffer {
            spell,
            usable,
            cost_copper,
            required_level,
            required_skill_line,
            required_skill_rank,
        });
    }
    Some(ProtocolObservation::TrainerList {
        trainer,
        trainer_type,
        offers,
    })
}

fn parse_vendor_list(body: &[u8]) -> Option<ProtocolObservation> {
    let vendor = EntityId(u64_le_at(body, 0)?);
    if vendor.0 == 0 {
        return None;
    }
    let count = usize::from(*body.get(8)?);
    let mut cursor = 9;
    let mut offers = Vec::with_capacity(count);
    for _ in 0..count {
        let slot = read_u32(body, &mut cursor)?;
        let item = read_u32(body, &mut cursor)?;
        let _display_id = read_u32(body, &mut cursor)?;
        let stock_raw = read_u32(body, &mut cursor)? as i32;
        let price_copper = read_u32(body, &mut cursor)?;
        let _max_durability = read_u32(body, &mut cursor)?;
        let buy_count = read_u32(body, &mut cursor)?;
        let extended_cost = read_u32(body, &mut cursor)?;
        if slot == 0 || item == 0 || buy_count == 0 {
            continue;
        }
        offers.push(wow_state::inventory::VendorOffer {
            slot,
            item,
            stock: (stock_raw >= 0).then_some(stock_raw as u32),
            price_copper,
            buy_count,
            extended_cost,
        });
    }
    Some(ProtocolObservation::VendorInventory { vendor, offers })
}

fn parse_vendor_buy_response(body: &[u8]) -> Option<ProtocolObservation> {
    let vendor = EntityId(u64_le_at(body, 0)?);
    let slot = u32_le_at(body, 8)?;
    let stock_raw = i32::from_le_bytes(body.get(12..16)?.try_into().ok()?);
    let purchased_lots = u32_le_at(body, 16)?;
    (vendor.0 != 0).then_some(ProtocolObservation::VendorStockUpdated {
        vendor,
        slot,
        stock: (stock_raw >= 0).then_some(stock_raw as u32),
        purchased_lots,
    })
}

fn parse_item_template(body: &[u8]) -> Option<ProtocolObservation> {
    let mut offset = 0usize;
    let item = read_u32(body, &mut offset)?;
    if item == 0 || item & 0x8000_0000 != 0 {
        return None;
    }
    let item_class = read_u32(body, &mut offset)?;
    let subclass = read_u32(body, &mut offset)?;
    read_u32(body, &mut offset)?; // sound override subclass
    let mut names = Vec::with_capacity(4);
    for _ in 0..4 {
        let end = body.get(offset..)?.iter().position(|byte| *byte == 0)? + offset;
        names.push(String::from_utf8_lossy(body.get(offset..end)?).into_owned());
        offset = end.checked_add(1)?;
    }
    offset = offset.checked_add(4)?; // display id
    let quality = read_u32(body, &mut offset)?;
    offset = offset.checked_add(8)?; // flags, flags2
    read_u32(body, &mut offset)?; // buy price
    let sell_price = read_u32(body, &mut offset)?;
    let inventory_type = read_u32(body, &mut offset)?;
    let allowable_class = read_u32(body, &mut offset)?;
    read_u32(body, &mut offset)?; // allowable race
    let item_level = read_u32(body, &mut offset)?;
    let required_level = read_u32(body, &mut offset)?;
    offset = offset.checked_add(4 * 9)?; // skill, spell, reputation and stack fields
    let container_slots = read_u32(body, &mut offset)?;
    let stat_count = read_u32(body, &mut offset)?.min(10) as usize;
    let mut stats = Vec::with_capacity(stat_count);
    for _ in 0..stat_count {
        let stat = read_u32(body, &mut offset)? as i32;
        let value = read_u32(body, &mut offset)? as i32;
        stats.push((stat, value));
    }
    offset = offset.checked_add(8)?; // scaling distribution and value
    let mut damage_min = f32::from_bits(read_u32(body, &mut offset)?);
    let mut damage_max = f32::from_bits(read_u32(body, &mut offset)?);
    read_u32(body, &mut offset)?; // damage type
    damage_min += f32::from_bits(read_u32(body, &mut offset)?);
    damage_max += f32::from_bits(read_u32(body, &mut offset)?);
    read_u32(body, &mut offset)?; // second damage type
    let armor = read_u32(body, &mut offset)?;
    offset = offset.checked_add(4 * 6)?; // resistances
    let delay_ms = read_u32(body, &mut offset)?;
    let ammo_type = read_u32(body, &mut offset)?;
    offset = offset.checked_add(4)?; // ranged modifier
    let mut use_spell_id = 0;
    for _ in 0..5 {
        let spell = read_u32(body, &mut offset)?;
        let trigger = read_u32(body, &mut offset)?;
        offset = offset.checked_add(16)?; // charges, cooldown and category fields
        if use_spell_id == 0 && spell != 0 && trigger == 0 {
            use_spell_id = spell;
        }
    }
    Some(ProtocolObservation::ItemTemplate {
        item,
        metadata: wow_state::inventory::ItemTemplateMetadata {
            name: names.into_iter().next().unwrap_or_default(),
            item_class,
            subclass,
            quality,
            sell_price,
            inventory_type,
            allowable_class,
            item_level,
            required_level,
            stats,
            armor,
            damage_min,
            damage_max,
            delay_ms,
            container_slots,
            ammo_type,
            use_spell_id,
        },
    })
}

fn parse_creature_killed(body: &[u8]) -> Option<ProtocolObservation> {
    let killer = EntityId(u64_le_at(body, 0)?);
    let victim = EntityId(u64_le_at(body, 8)?);
    (killer.0 != 0 && victim.0 != 0)
        .then_some(ProtocolObservation::CreatureKilled { killer, victim })
}

fn parse_spell_start(body: &[u8]) -> Option<ProtocolObservation> {
    let mut offset = 0usize;
    read_packed_guid(body, &mut offset)?; // cast item or caster
    let caster = EntityId(read_packed_guid(body, &mut offset)?);
    offset = offset.checked_add(1)?; // cast count
    let spell = read_u32(body, &mut offset)?;
    offset = offset.checked_add(4)?; // cast flags
    let remaining_ms = read_u32(body, &mut offset)?;
    cast_started(caster, spell, remaining_ms)
}

fn parse_spell_go(body: &[u8]) -> Option<ProtocolObservation> {
    let mut offset = 0usize;
    read_packed_guid(body, &mut offset)?; // cast item or caster
    let caster = EntityId(read_packed_guid(body, &mut offset)?);
    offset = offset.checked_add(1)?; // cast count
    let spell = read_u32(body, &mut offset)?;
    (caster.0 != 0 && spell != 0).then_some(ProtocolObservation::CastFinished { caster, spell })
}

fn parse_channel_start(body: &[u8]) -> Option<ProtocolObservation> {
    let mut offset = 0usize;
    let caster = EntityId(read_packed_guid(body, &mut offset)?);
    let spell = read_u32(body, &mut offset)?;
    let duration_ms = read_u32(body, &mut offset)?;
    cast_started(caster, spell, duration_ms)
}

fn cast_started(caster: EntityId, spell: u32, duration_ms: u32) -> Option<ProtocolObservation> {
    if caster.0 == 0 || spell == 0 || duration_ms == 0 {
        return None;
    }
    let started_at_ms = Millis::wall_clock_now().0;
    Some(ProtocolObservation::CastStarted {
        caster,
        spell,
        started_at_ms,
        ends_at_ms: Millis(started_at_ms)
            .saturating_add(u64::from(duration_ms))
            .0,
    })
}

fn parse_channel_update(body: &[u8]) -> Option<ProtocolObservation> {
    let mut offset = 0usize;
    let caster = EntityId(read_packed_guid(body, &mut offset)?);
    let remaining_ms = read_u32(body, &mut offset)?;
    if caster.0 == 0 {
        return None;
    }
    let ends_at_ms = if remaining_ms == 0 {
        0
    } else {
        Millis::wall_clock_now()
            .saturating_add(u64::from(remaining_ms))
            .0
    };
    Some(ProtocolObservation::CastUpdated { caster, ends_at_ms })
}

pub(super) fn parse_corpse_query(body: &[u8]) -> Option<ProtocolObservation> {
    if *body.first()? == 0 {
        return Some(ProtocolObservation::CorpseLocation { position: None });
    }
    let map = u32_le_at(body, 1)?;
    let x = f32::from_bits(u32_le_at(body, 5)?);
    let y = f32::from_bits(u32_le_at(body, 9)?);
    let z = f32::from_bits(u32_le_at(body, 13)?);
    let point = Vec3::new(x, y, z);
    if !point.is_finite() {
        return None;
    }
    Some(ProtocolObservation::CorpseLocation {
        position: Some(WorldPosition {
            map,
            point,
            orientation: 0.0,
        }),
    })
}
pub(super) fn parse_reclaim_delay(body: &[u8]) -> Option<ProtocolObservation> {
    let delay = u32_le_at(body, 0)?;
    Some(ProtocolObservation::CorpseReclaimDelay {
        ready_at_ms: Millis::wall_clock_now().saturating_add(u64::from(delay)).0,
    })
}

pub(super) fn parse_spell_cooldowns(body: &[u8]) -> Vec<ProtocolObservation> {
    if body.len() < 9 {
        return Vec::new();
    }
    let now = Millis::wall_clock_now().0;
    let mut offset = 9usize; // caster GUID + flags
    let starts_global_cooldown = body[8] & 0x01 != 0;
    let mut out = Vec::new();
    while offset + 8 <= body.len() {
        let spell = u32_le_at(body, offset).unwrap_or_default();
        let cooldown = u32_le_at(body, offset + 4).unwrap_or_default();
        offset += 8;
        if spell != 0 {
            out.push(ProtocolObservation::SpellCooldown {
                spell,
                ready_at_ms: Millis(now).saturating_add(u64::from(cooldown)).0,
            });
            if starts_global_cooldown {
                out.push(ProtocolObservation::SpellGlobalCooldown {
                    spell,
                    started_at_ms: now,
                });
            }
        }
    }
    out
}

pub(super) fn parse_player_runes(body: &[u8]) -> Option<Vec<wow_state::capabilities::RuneState>> {
    if body.len() < 4 {
        return None;
    }
    let count = u32_le_at(body, 0)? as usize;
    if count != 6 || body.len() != 4 + count * 2 {
        return None;
    }
    let mut runes = Vec::with_capacity(count);
    for pair in body[4..].chunks_exact(2) {
        let rune_type = pair[0];
        if rune_type > 3 {
            return None;
        }
        runes.push(wow_state::capabilities::RuneState {
            rune_type,
            ready: pair[1] == u8::MAX,
        });
    }
    Some(runes)
}

pub(super) fn parse_combo_points(body: &[u8]) -> ProtocolObservation {
    let mut offset = 0;
    let target = read_packed_guid(body, &mut offset).map(EntityId);
    let points = body
        .get(offset)
        .copied()
        .filter(|_| offset + 1 == body.len());
    ProtocolObservation::ComboPoints { target, points }
}

pub(super) fn parse_initial_spells(body: &[u8]) -> Vec<ProtocolObservation> {
    if body.len() < 3 {
        return Vec::new();
    }
    let Some(count) = wow_domain::binary::u16_le_at(body, 1).map(|count| count as usize) else {
        return Vec::new();
    };
    if count > 4096 || body.len() < 3 + count.saturating_mul(6) {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(count);
    let mut offset = 3usize;
    for _ in 0..count {
        let spell = u32_le_at(body, offset).unwrap_or_default();
        offset += 6;
        if spell != 0 {
            out.push(ProtocolObservation::SpellKnown { spell });
        }
    }
    out
}

pub(super) fn parse_aura_update(body: &[u8]) -> Option<ProtocolObservation> {
    let mut offset = 0usize;
    let entity = EntityId(read_packed_guid(body, &mut offset)?);
    let slot = *body.get(offset)?;
    offset += 1;
    let aura = parse_aura_entry(body, &mut offset, slot)?;
    Some(ProtocolObservation::AuraSlot { entity, slot, aura })
}

pub(super) fn parse_aura_update_all(body: &[u8]) -> Option<ProtocolObservation> {
    let mut offset = 0usize;
    let entity = EntityId(read_packed_guid(body, &mut offset)?);
    let mut auras = Vec::new();
    while offset < body.len() {
        let slot = *body.get(offset)?;
        offset += 1;
        if let Some(aura) = parse_aura_entry(body, &mut offset, slot)? {
            auras.push(aura);
        }
    }
    Some(ProtocolObservation::AuraSnapshot { entity, auras })
}

fn parse_aura_entry(
    body: &[u8],
    offset: &mut usize,
    slot: u8,
) -> Option<Option<wow_state::auras::AuraInstance>> {
    let spell = u32_le_at(body, *offset)?;
    *offset += 4;
    if spell == 0 {
        return Some(None);
    }
    let flags = *body.get(*offset)?;
    *offset += 1;
    body.get(*offset..offset.checked_add(2)?)?;
    *offset += 2; // aura level and applications/stacks
    let caster = if flags & 0x08 == 0 {
        Some(EntityId(read_packed_guid(body, offset)?))
    } else {
        None
    };
    let (max_duration_ms, remaining_ms, observed_at_ms) = if flags & 0x20 != 0 {
        let max = u32_le_at(body, *offset)? as i32;
        *offset += 4;
        let remaining = u32_le_at(body, *offset)? as i32;
        *offset += 4;
        (
            (max >= 0).then_some(max as u32),
            (remaining >= 0).then_some(remaining as u32),
            (remaining >= 0).then(|| wow_domain::time::Millis::wall_clock_now().0),
        )
    } else {
        (None, None, None)
    };
    if flags & 0x40 != 0 {
        for effect in 0..3 {
            if flags & (1 << effect) != 0 {
                body.get(*offset..offset.checked_add(4)?)?;
                *offset += 4;
            }
        }
    }
    Some(Some(wow_state::auras::AuraInstance {
        slot,
        spell,
        positive: Some(flags & 0x10 != 0),
        caster,
        max_duration_ms,
        remaining_ms,
        observed_at_ms,
    }))
}

pub(super) fn controlled_abilities_observation(
    opcode: u32,
    body: &[u8],
) -> Option<ProtocolObservation> {
    const SMSG_PET_SPELLS: u32 = 0x0179;
    if opcode != SMSG_PET_SPELLS {
        return None;
    }
    let packet = wow_state::pets::parse_controlled_unit_spell_bar(body)?;
    let mover = packet.mover?;
    let spells = packet
        .abilities
        .into_iter()
        .map(|ability| ability.spell)
        .collect();
    Some(ProtocolObservation::ControlledAbilities { mover, spells })
}

pub(super) fn quest_observations(opcode: u32, body: &[u8]) -> Vec<ProtocolObservation> {
    const SMSG_QUESTGIVER_STATUS: u32 = 0x0183;
    const SMSG_QUESTGIVER_QUEST_LIST: u32 = 0x0185;
    const SMSG_GOSSIP_MESSAGE: u32 = 0x017D;
    const SMSG_QUESTGIVER_REQUEST_ITEMS: u32 = 0x018B;
    const SMSG_QUESTGIVER_OFFER_REWARD: u32 = 0x018D;
    const SMSG_QUESTGIVER_QUEST_COMPLETE: u32 = 0x018F;
    const SMSG_QUESTGIVER_STATUS_MULTIPLE: u32 = 0x0418;
    const SMSG_QUEST_QUERY_RESPONSE: u32 = 0x005D;
    match opcode {
        SMSG_QUESTGIVER_STATUS => parse_single_quest_status(body).into_iter().collect(),
        SMSG_QUESTGIVER_STATUS_MULTIPLE => parse_multiple_quest_status(body),
        SMSG_QUESTGIVER_QUEST_LIST => parse_quest_list(body),
        SMSG_GOSSIP_MESSAGE => parse_gossip_message(body),
        SMSG_QUESTGIVER_REQUEST_ITEMS => parse_quest_request_items(body).into_iter().collect(),
        SMSG_QUESTGIVER_OFFER_REWARD => parse_quest_offer_reward(body).into_iter().collect(),
        // The questgiver completion packet is not enough to confirm that the
        // quest left the active journal. The quest journal remains the source
        // of truth for completed quests.
        SMSG_QUESTGIVER_QUEST_COMPLETE => Vec::new(),
        SMSG_QUEST_QUERY_RESPONSE => parse_quest_query_response(body).into_iter().collect(),
        _ => Vec::new(),
    }
}

pub(super) fn parse_single_quest_status(body: &[u8]) -> Option<ProtocolObservation> {
    parse_quest_status_entry(body, 0)
}

pub(super) fn parse_multiple_quest_status(body: &[u8]) -> Vec<ProtocolObservation> {
    let Some(count) = u32_le_at(body, 0).map(|count| count as usize) else {
        return Vec::new();
    };
    if count > 4096 || body.len() < 4 + count.saturating_mul(9) {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(count);
    let mut offset = 4;
    for _ in 0..count {
        let Some(observation) = parse_quest_status_entry(body, offset) else {
            return Vec::new();
        };
        out.push(observation);
        offset += 9;
    }
    out
}

pub(super) fn parse_quest_list(body: &[u8]) -> Vec<ProtocolObservation> {
    let Some(giver) = read_guid(body, 0) else {
        return Vec::new();
    };
    let mut offset = 8;
    if read_cstring(body, &mut offset).is_none() {
        return Vec::new();
    }
    if body.get(offset..offset + 8).is_none() {
        return Vec::new();
    }
    offset += 8; // emote delay + emote type
    let Some(count) = body.get(offset).copied() else {
        return Vec::new();
    };
    offset += 1;
    let mut offers = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let Some((quest, icon)) = parse_quest_offer_record(body, &mut offset) else {
            return Vec::new();
        };
        offers.push(ProtocolObservation::QuestOffer { giver, quest, icon });
    }
    let mut out = Vec::with_capacity(offers.len() + 1);
    out.push(ProtocolObservation::QuestGiverListReceived {
        giver,
        offer_count: count,
    });
    out.extend(offers);
    out
}

pub(super) fn parse_gossip_message(body: &[u8]) -> Vec<ProtocolObservation> {
    const MAX_GOSSIP_ENTRIES: usize = 64;

    let Some(giver) = read_guid(body, 0) else {
        return Vec::new();
    };
    let mut offset = 8usize;
    if body.get(offset..offset + 8).is_none() {
        return Vec::new();
    }
    offset += 8; // gossip menu ID and NPC text ID

    let Some(gossip_count) = u32_le_at(body, offset).map(|count| count as usize) else {
        return Vec::new();
    };
    if gossip_count > MAX_GOSSIP_ENTRIES {
        return Vec::new();
    }
    offset += 4;
    for _ in 0..gossip_count {
        if body.get(offset..offset + 10).is_none() {
            return Vec::new();
        }
        offset += 10; // option index, icon, coded, and box money
        if read_cstring(body, &mut offset).is_none() || read_cstring(body, &mut offset).is_none() {
            return Vec::new();
        }
    }

    let Some(quest_count) = u32_le_at(body, offset).map(|count| count as usize) else {
        return Vec::new();
    };
    if quest_count > MAX_GOSSIP_ENTRIES {
        return Vec::new();
    }
    offset += 4;

    let mut observations = Vec::with_capacity(quest_count + 1);
    observations.push(ProtocolObservation::QuestGiverListReceived {
        giver,
        offer_count: quest_count as u8,
    });
    for _ in 0..quest_count {
        let Some((quest, icon)) = parse_quest_offer_record(body, &mut offset) else {
            return Vec::new();
        };
        if quest != 0 {
            observations.push(ProtocolObservation::QuestOffer { giver, quest, icon });
        }
    }
    observations
}

fn parse_quest_offer_record(body: &[u8], offset: &mut usize) -> Option<(u32, u32)> {
    let quest = u32_le_at(body, *offset)?;
    let icon = u32_le_at(body, offset.checked_add(4)?)?;
    body.get(*offset..offset.checked_add(17)?)?;
    *offset += 17;
    read_cstring(body, offset)?;
    Some((quest, icon))
}

pub(super) fn parse_quest_request_items(body: &[u8]) -> Option<ProtocolObservation> {
    use wow_state::quests::{QuestTurnInDialog, QuestTurnInStage};
    let giver = read_guid(body, 0)?;
    let quest = u32_le_at(body, 8)?;
    if body.len() < 28 {
        return None;
    }
    let completion_code = u32_le_at(body, body.len().checked_sub(16)?)?;
    Some(ProtocolObservation::QuestTurnInDialog {
        quest,
        dialog: QuestTurnInDialog {
            giver,
            stage: QuestTurnInStage::RequestItems {
                can_complete: completion_code == 3,
            },
        },
    })
}

pub(super) fn parse_quest_offer_reward(body: &[u8]) -> Option<ProtocolObservation> {
    use wow_state::quests::{QuestTurnInDialog, QuestTurnInStage};
    let giver = read_guid(body, 0)?;
    let quest = u32_le_at(body, 8)?;
    let mut offset = 12usize;
    let _title = read_cstring(body, &mut offset)?;
    let _reward_text = read_cstring(body, &mut offset)?;
    offset = offset.checked_add(1 + 4 + 4)?;
    let emote_count = u32_le_at(body, offset)? as usize;
    if emote_count > 64 {
        return None;
    }
    offset = offset.checked_add(4 + emote_count.checked_mul(8)?)?;
    let choice_count = u32_le_at(body, offset)? as usize;
    if choice_count > 6 {
        return None;
    }
    offset = offset.checked_add(4)?;
    let mut reward_items = Vec::with_capacity(choice_count);
    for _ in 0..choice_count {
        let item = u32_le_at(body, offset)?;
        let count = u32_le_at(body, offset.checked_add(4)?)?;
        // The final word is the client display id. It is not needed by policy,
        // but its presence is part of each packet record.
        u32_le_at(body, offset.checked_add(8)?)?;
        if item == 0 || count == 0 {
            return None;
        }
        reward_items.push(item);
        offset = offset.checked_add(12)?;
    }
    Some(ProtocolObservation::QuestTurnInDialog {
        quest,
        dialog: QuestTurnInDialog {
            giver,
            stage: QuestTurnInStage::OfferReward { reward_items },
        },
    })
}

pub(super) fn parse_quest_query_response(body: &[u8]) -> Option<ProtocolObservation> {
    use wow_state::quests::{
        QuestDefinition, QuestItemObjective, QuestTargetKind, QuestTargetObjective,
    };
    // AzerothCore 3.3.5a writes 65 four-byte fields before the five strings.
    if body.len() < 260 {
        return None;
    }
    let read_u32 = |index: usize| -> Option<u32> {
        let offset = index.checked_mul(4)?;
        u32_le_at(body, offset)
    };
    let quest = read_u32(0)?;
    let poi_map_raw = read_u32(61)?;
    let poi_x = f32::from_bits(read_u32(62)?);
    let poi_y = f32::from_bits(read_u32(63)?);
    let mut offset = 260usize;
    let title = read_cstring(body, &mut offset)?.to_owned();
    let _objectives = read_cstring(body, &mut offset)?;
    let _details = read_cstring(body, &mut offset)?;
    let _area = read_cstring(body, &mut offset)?;
    let _completed = read_cstring(body, &mut offset)?;

    let mut raw_targets = Vec::with_capacity(4);
    for _ in 0..4 {
        let encoded = u32_le_at(body, offset)?;
        offset += 4;
        let required = u32_le_at(body, offset)?;
        offset += 4;
        let item_drop = u32_le_at(body, offset)?;
        offset += 4;
        let _source_count = u32_le_at(body, offset)?;
        offset += 4;
        let (kind, entry) = if encoded & 0x8000_0000 != 0 {
            (QuestTargetKind::GameObject, encoded & 0x7FFF_FFFF)
        } else {
            (QuestTargetKind::Creature, encoded)
        };
        raw_targets.push((kind, entry, required, item_drop));
    }
    let mut items = Vec::new();
    for _ in 0..6 {
        let item = u32_le_at(body, offset)?;
        offset += 4;
        let required = u32_le_at(body, offset)?;
        offset += 4;
        if item != 0 && required != 0 {
            items.push(QuestItemObjective { item, required });
        }
    }
    let mut texts = Vec::with_capacity(4);
    for _ in 0..4 {
        texts.push(
            read_cstring(body, &mut offset)
                .unwrap_or_default()
                .to_owned(),
        );
    }
    let targets = raw_targets
        .into_iter()
        .enumerate()
        .filter_map(|(i, (kind, entry, required, item_drop))| {
            (entry != 0 && required != 0).then(|| QuestTargetObjective {
                slot: i,
                kind,
                entry,
                required,
                item_drop,
                text: texts.get(i).cloned().unwrap_or_default(),
            })
        })
        .collect();
    let poi_valid = poi_map_raw != u32::MAX
        && poi_x.is_finite()
        && poi_y.is_finite()
        && (poi_x != 0.0 || poi_y != 0.0);
    Some(ProtocolObservation::QuestDefinition {
        definition: QuestDefinition {
            quest,
            title,
            poi_map: poi_valid.then_some(poi_map_raw),
            poi_x: poi_valid.then_some(poi_x),
            poi_y: poi_valid.then_some(poi_y),
            targets,
            items,
        },
    })
}

pub(super) fn read_cstring<'a>(body: &'a [u8], offset: &mut usize) -> Option<&'a str> {
    let (text, next) = cstring_at(body, *offset, None).ok()?;
    *offset = next;
    Some(text)
}

fn read_guid(body: &[u8], offset: usize) -> Option<EntityId> {
    Some(EntityId(u64_le_at(body, offset)?))
}

fn parse_quest_status_entry(body: &[u8], offset: usize) -> Option<ProtocolObservation> {
    let giver = read_guid(body, offset)?;
    let status = *body.get(offset.checked_add(8)?)?;
    Some(ProtocolObservation::QuestGiverStatus { giver, status })
}
