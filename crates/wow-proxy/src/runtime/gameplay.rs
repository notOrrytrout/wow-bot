use anyhow::Result;
use std::time::Duration;
use wow_domain::{EntityId, GameplayCommand, Vec3, WorldPosition};
use wow_srp::wrath_header::ServerEncrypterHalf;

use crate::framing::{ClientFrame, ServerFrame, upstream_edge::write_server_frame};

pub(super) fn parse_cast_failed(body: &[u8]) -> Option<(u8, u32, u8)> {
    // AzerothCore Spell::WriteCastResultInfo: cast_count:u8, spell:u32, reason:u8.
    let cast_count = *body.first()?;
    let spell = u32::from_le_bytes(body.get(1..5)?.try_into().ok()?);
    let reason = *body.get(5)?;
    Some((cast_count, spell, reason))
}

pub(super) fn take_bot_cast_failure(
    body: &[u8],
    last_bot_cast: &mut Option<(u32, Option<EntityId>, std::time::Instant)>,
) -> Option<(u8, u32, u8, Option<EntityId>)> {
    let (cast_count, spell, reason) = parse_cast_failed(body)?;
    let target = last_bot_cast
        .take()
        .filter(|(pending_spell, _, at)| {
            *pending_spell == spell && at.elapsed() < Duration::from_secs(5)
        })
        .and_then(|(_, target, _)| target);
    Some((cast_count, spell, reason, target))
}

pub(super) fn parse_loot_response(body: &[u8]) -> Option<(u64, u32, Vec<u8>)> {
    // Wrath SMSG_LOOT_RESPONSE: guid:u64, loot_type:u8, gold:u32,
    // item_count:u8, followed by 22-byte item records beginning with slot:u8.
    let guid = u64::from_le_bytes(body.get(0..8)?.try_into().ok()?);
    let gold = u32::from_le_bytes(body.get(9..13)?.try_into().ok()?);
    let count = usize::from(*body.get(13)?);
    let mut cursor = 14usize;
    let mut slots = Vec::with_capacity(count);
    for _ in 0..count {
        let slot = *body.get(cursor)?;
        body.get(cursor..cursor + 22)?;
        slots.push(slot);
        cursor += 22;
    }
    Some((guid, gold, slots))
}

pub(super) async fn write_bot_notice<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    encrypter: &mut ServerEncrypterHalf,
    message: &str,
) -> Result<()> {
    const SMSG_MESSAGECHAT_OPCODE: u16 = 0x0096;
    let mut body = Vec::with_capacity(32 + message.len());
    body.push(0); // CHAT_MSG_SYSTEM
    body.extend_from_slice(&0_u32.to_le_bytes()); // LANG_UNIVERSAL
    body.extend_from_slice(&0_u64.to_le_bytes()); // sender GUID
    body.extend_from_slice(&0_u32.to_le_bytes());
    body.extend_from_slice(&0_u64.to_le_bytes()); // target GUID
    body.extend_from_slice(&(u32::try_from(message.len() + 1).unwrap_or(u32::MAX)).to_le_bytes());
    body.extend_from_slice(message.as_bytes());
    body.push(0);
    body.push(0);
    write_server_frame(
        writer,
        encrypter,
        &ServerFrame {
            opcode: SMSG_MESSAGECHAT_OPCODE,
            body,
        },
    )
    .await
}

pub(super) fn encode_gameplay_command(
    command: GameplayCommand,
    player_guid: Option<EntityId>,
    current: Option<WorldPosition>,
    base_movement_flags: u32,
    movement_time: &mut u32,
) -> std::result::Result<Option<(ClientFrame, Option<(WorldPosition, bool, u32, u32)>)>, String> {
    const CMSG_USE_ITEM: u32 = 0x00AB;
    const CMSG_GAMEOBJ_USE: u32 = 0x00B1;
    const MSG_MOVE_STOP: u32 = 0x00B7;
    const MSG_MOVE_SET_FACING: u32 = 0x00DA;
    const MSG_MOVE_HEARTBEAT: u32 = 0x00EE;
    const CMSG_ATTACKSWING: u32 = 0x0141;
    const CMSG_LOOT: u32 = 0x015D;
    const CMSG_QUESTGIVER_HELLO: u32 = 0x0184;
    const CMSG_QUESTGIVER_ACCEPT_QUEST: u32 = 0x0189;
    const CMSG_QUESTGIVER_COMPLETE_QUEST: u32 = 0x018A;
    const CMSG_QUESTGIVER_REQUEST_REWARD: u32 = 0x018C;
    const CMSG_QUESTGIVER_CHOOSE_REWARD: u32 = 0x018E;
    const CMSG_QUESTGIVER_STATUS_MULTIPLE_QUERY: u32 = 0x0417;
    const CMSG_QUEST_QUERY: u32 = 0x005C;
    match command {
        GameplayCommand::Raw { opcode, body } => Ok(Some((ClientFrame { opcode, body }, None))),
        GameplayCommand::QueryQuestGivers => Ok(Some((
            ClientFrame {
                opcode: CMSG_QUESTGIVER_STATUS_MULTIPLE_QUERY,
                body: Vec::new(),
            },
            None,
        ))),
        GameplayCommand::QueryQuest { quest } => Ok(Some((
            ClientFrame {
                opcode: CMSG_QUEST_QUERY,
                body: quest.to_le_bytes().to_vec(),
            },
            None,
        ))),
        GameplayCommand::Interact(entity) => Ok(Some((
            ClientFrame {
                opcode: CMSG_QUESTGIVER_HELLO,
                body: raw_guid_body(entity),
            },
            None,
        ))),
        GameplayCommand::UseGameObject(entity) => Ok(Some((
            ClientFrame {
                opcode: CMSG_GAMEOBJ_USE,
                body: raw_guid_body(entity),
            },
            None,
        ))),
        GameplayCommand::CastGameObject { spell, target, .. } => Ok(Some((
            ClientFrame {
                opcode: 0x012E,
                body: encode_cast_spell(spell, Some(target), 0x0000_0800),
            },
            None,
        ))),
        GameplayCommand::UseItemInstance {
            item: _,
            item_guid,
            backpack_slot,
            spell,
            target,
            cast_count,
        } => {
            let mut body = Vec::with_capacity(32);
            body.push(0xff); // INVENTORY_SLOT_BAG_0
            body.push(backpack_slot);
            body.push(cast_count);
            body.extend_from_slice(&spell.to_le_bytes());
            append_raw_guid(&mut body, item_guid);
            body.extend_from_slice(&0_u32.to_le_bytes()); // glyph index
            body.push(0); // cast flags
            match target {
                Some(target) => {
                    body.extend_from_slice(&0x0000_0002_u32.to_le_bytes());
                    push_packed_guid(&mut body, target);
                }
                None => body.extend_from_slice(&0_u32.to_le_bytes()),
            }
            Ok(Some((
                ClientFrame {
                    opcode: CMSG_USE_ITEM,
                    body,
                },
                None,
            )))
        }
        GameplayCommand::Attack(entity) => Ok(Some((
            ClientFrame {
                opcode: CMSG_ATTACKSWING,
                body: raw_guid_body(entity),
            },
            None,
        ))),
        GameplayCommand::Loot(entity) => Ok(Some((
            ClientFrame {
                opcode: CMSG_LOOT,
                body: raw_guid_body(entity),
            },
            None,
        ))),
        GameplayCommand::AcceptQuest { quest, giver } => Ok(Some((
            ClientFrame {
                opcode: CMSG_QUESTGIVER_ACCEPT_QUEST,
                body: encode_questgiver_request(giver, quest, Some(0)),
            },
            None,
        ))),
        GameplayCommand::TurnInQuest { quest, giver } => Ok(Some((
            ClientFrame {
                opcode: CMSG_QUESTGIVER_COMPLETE_QUEST,
                body: encode_questgiver_request(giver, quest, None),
            },
            None,
        ))),
        GameplayCommand::RequestQuestReward { quest, giver } => Ok(Some((
            ClientFrame {
                opcode: CMSG_QUESTGIVER_REQUEST_REWARD,
                body: encode_questgiver_request(giver, quest, None),
            },
            None,
        ))),
        GameplayCommand::ChooseQuestReward {
            quest,
            giver,
            reward,
        } => Ok(Some((
            ClientFrame {
                opcode: CMSG_QUESTGIVER_CHOOSE_REWARD,
                body: encode_questgiver_request(giver, quest, Some(reward)),
            },
            None,
        ))),
        GameplayCommand::MaintainBuff { spell, target } => Ok(Some((
            ClientFrame {
                opcode: 0x012E,
                body: encode_cast_spell(spell, Some(target), 0x0000_0002),
            },
            None,
        ))),
        GameplayCommand::Cast { spell, target }
        | GameplayCommand::VehicleCast { spell, target } => Ok(Some((
            ClientFrame {
                opcode: 0x012E,
                body: encode_cast_spell(spell, target, 0x0000_0002),
            },
            None,
        ))),
        GameplayCommand::FaceDirection { orientation } => {
            let guid = player_guid
                .ok_or_else(|| "facing requested before player GUID is authoritative".to_owned())?;
            let mut position = current
                .ok_or_else(|| "facing requested before canonical position is known".to_owned())?;
            if !orientation.is_finite() {
                return Err("facing orientation is invalid".into());
            }
            position.orientation = orientation.rem_euclid(std::f32::consts::TAU);
            *movement_time = movement_time.wrapping_add(1).max(1);
            let flags = base_movement_flags & !0x0000_0001_u32;
            let body = encode_simple_movement(
                guid,
                flags,
                *movement_time,
                position.point,
                position.orientation,
            );
            Ok(Some((
                ClientFrame {
                    opcode: MSG_MOVE_SET_FACING,
                    body,
                },
                Some((position, false, flags, *movement_time)),
            )))
        }
        GameplayCommand::ReleaseSpirit => Ok(Some((
            ClientFrame {
                opcode: 0x015A,
                body: vec![0],
            },
            None,
        ))),
        GameplayCommand::QueryCorpse => Ok(Some((
            ClientFrame {
                opcode: 0x0216,
                body: Vec::new(),
            },
            None,
        ))),
        GameplayCommand::ReclaimCorpse { player } => Ok(Some((
            ClientFrame {
                opcode: 0x01D2,
                body: raw_guid_body(player),
            },
            None,
        ))),
        GameplayCommand::MoveTo(destination) => {
            let guid = player_guid.ok_or_else(|| {
                "movement requested before player GUID is authoritative".to_owned()
            })?;
            let mut position = current.ok_or_else(|| {
                "movement requested before canonical position is known".to_owned()
            })?;
            if !destination.is_finite() {
                return Err("movement destination is invalid".into());
            }
            let dx = destination.x - position.point.x;
            let dy = destination.y - position.point.y;
            if dx != 0.0 || dy != 0.0 {
                position.orientation = dy.atan2(dx);
            }
            position.point = destination;
            *movement_time = movement_time.wrapping_add(250).max(1);
            // Preserve server-authoritative capabilities such as flying/disable-gravity,
            // while setting forward movement for this step.
            let flags = base_movement_flags | 0x0000_0001_u32;
            let body = encode_simple_movement(
                guid,
                flags,
                *movement_time,
                position.point,
                position.orientation,
            );
            Ok(Some((
                ClientFrame {
                    opcode: MSG_MOVE_HEARTBEAT,
                    body,
                },
                Some((position, true, flags, *movement_time)),
            )))
        }
        GameplayCommand::StopMovement => {
            let guid = player_guid.ok_or_else(|| {
                "stop movement requested before player GUID is authoritative".to_owned()
            })?;
            let position = current.ok_or_else(|| {
                "stop movement requested before canonical position is known".to_owned()
            })?;
            *movement_time = movement_time.wrapping_add(1).max(1);
            let flags = base_movement_flags & !0x0000_0001_u32;
            let body = encode_simple_movement(
                guid,
                flags,
                *movement_time,
                position.point,
                position.orientation,
            );
            Ok(Some((
                ClientFrame {
                    opcode: MSG_MOVE_STOP,
                    body,
                },
                Some((position, false, flags, *movement_time)),
            )))
        }
        other => Err(format!("{other:?}")),
    }
}

fn encode_cast_spell(spell: u32, target: Option<EntityId>, target_flag: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(24);
    body.push(0); // cast count
    body.extend_from_slice(&spell.to_le_bytes());
    body.push(0); // cast flags
    if let Some(target) = target {
        body.extend_from_slice(&target_flag.to_le_bytes());
        push_packed_guid(&mut body, target);
    } else {
        body.extend_from_slice(&0_u32.to_le_bytes());
    }
    body
}

fn encode_questgiver_request(giver: EntityId, quest: u32, trailing_value: Option<u32>) -> Vec<u8> {
    let mut body = Vec::with_capacity(if trailing_value.is_some() { 16 } else { 12 });
    append_raw_guid(&mut body, giver);
    body.extend_from_slice(&quest.to_le_bytes());
    if let Some(value) = trailing_value {
        body.extend_from_slice(&value.to_le_bytes());
    }
    body
}

fn raw_guid_body(guid: EntityId) -> Vec<u8> {
    let mut body = Vec::with_capacity(8);
    append_raw_guid(&mut body, guid);
    body
}

fn append_raw_guid(body: &mut Vec<u8>, guid: EntityId) {
    body.extend_from_slice(&guid.0.to_le_bytes());
}

pub(super) fn push_packed_guid(body: &mut Vec<u8>, guid: EntityId) {
    let bytes = guid.0.to_le_bytes();
    let mut mask = 0_u8;
    let start = body.len();
    body.push(0);
    for (index, byte) in bytes.into_iter().enumerate() {
        if byte != 0 {
            mask |= 1 << index;
            body.push(byte);
        }
    }
    body[start] = mask;
}

pub(super) fn encode_simple_movement(
    guid: EntityId,
    flags: u32,
    client_time: u32,
    point: Vec3,
    orientation: f32,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(45);
    push_packed_guid(&mut body, guid);
    body.extend_from_slice(&flags.to_le_bytes());
    body.extend_from_slice(&0_u16.to_le_bytes());
    body.extend_from_slice(&client_time.to_le_bytes());
    body.extend_from_slice(&point.x.to_le_bytes());
    body.extend_from_slice(&point.y.to_le_bytes());
    body.extend_from_slice(&point.z.to_le_bytes());
    body.extend_from_slice(&orientation.to_le_bytes());
    body.extend_from_slice(&0_u32.to_le_bytes());
    body
}

pub(super) fn decode_simple_movement(body: &[u8]) -> Option<(EntityId, u32, u32, Vec3, f32)> {
    let mask = *body.first()?;
    let mut cursor = 1usize;
    let mut guid = [0_u8; 8];
    for (index, slot) in guid.iter_mut().enumerate() {
        if mask & (1 << index) != 0 {
            *slot = *body.get(cursor)?;
            cursor += 1;
        }
    }
    let flags = u32::from_le_bytes(body.get(cursor..cursor + 4)?.try_into().ok()?);
    cursor += 4;
    let _extra = u16::from_le_bytes(body.get(cursor..cursor + 2)?.try_into().ok()?);
    cursor += 2;
    let client_time = u32::from_le_bytes(body.get(cursor..cursor + 4)?.try_into().ok()?);
    cursor += 4;
    let x = f32::from_le_bytes(body.get(cursor..cursor + 4)?.try_into().ok()?);
    cursor += 4;
    let y = f32::from_le_bytes(body.get(cursor..cursor + 4)?.try_into().ok()?);
    cursor += 4;
    let z = f32::from_le_bytes(body.get(cursor..cursor + 4)?.try_into().ok()?);
    cursor += 4;
    let o = f32::from_le_bytes(body.get(cursor..cursor + 4)?.try_into().ok()?);
    let point = Vec3::new(x, y, z);
    if !point.is_finite() || !o.is_finite() {
        return None;
    }
    Some((
        EntityId(u64::from_le_bytes(guid)),
        flags,
        client_time,
        point,
        o,
    ))
}

pub(super) fn movement_matches_bot_visual(
    visual: WorldPosition,
    bot_opcode: u32,
    point: Vec3,
    orientation: f32,
    client_opcode: u32,
) -> bool {
    if !point.is_finite() || !orientation.is_finite() {
        return false;
    }
    let delta = (visual.orientation - orientation).rem_euclid(std::f32::consts::TAU);
    let angular = delta.min(std::f32::consts::TAU - delta);
    point.distance(visual.point) <= 0.25
        && angular <= 0.08
        && (client_opcode == bot_opcode || client_opcode == 0x00EE)
}

pub(super) fn is_player_movement_opcode(opcode: u32) -> bool {
    matches!(
        opcode,
        0x0B5
            | 0x0B6
            | 0x0B7
            | 0x0B8
            | 0x0B9
            | 0x0BA
            | 0x0BB
            | 0x0BC
            | 0x0BD
            | 0x0BE
            | 0x0BF
            | 0x0C0
            | 0x0C1
            | 0x0C2
            | 0x0C3
            | 0x0C5
            | 0x0C9
            | 0x0CA
            | 0x0CB
            | 0x0DA
            | 0x0DB
            | 0x0EE
            | 0x359
            | 0x35A
            | 0x3A7
    )
}

pub(super) fn is_explicit_player_movement_intent(opcode: u32) -> bool {
    // Start/change opcodes represent direct keyboard/mouse intent. Heartbeat,
    // stop, fall-land, and run/walk-mode packets are state feedback and are
    // not sufficient by themselves to steal locomotion from an active bot.
    matches!(
        opcode,
        0x0B5
            | 0x0B6
            | 0x0B8
            | 0x0B9
            | 0x0BB
            | 0x0BC
            | 0x0BD
            | 0x0BF
            | 0x0C0
            | 0x0CA
            | 0x0DA
            | 0x0DB
            | 0x359
            | 0x3A7
    )
}
