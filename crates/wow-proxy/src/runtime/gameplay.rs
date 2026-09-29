use anyhow::Result;
use std::time::{Duration, Instant};
use wow_domain::orientation::Radians;
use wow_domain::{EntityId, GameplayCommand, Vec3, WorldPosition};
use wow_srp::wrath_header::ServerEncrypterHalf;

use super::packet::{
    read_f32_cursor as read_f32, read_packed_guid, read_u16_cursor as read_u16,
    read_u32_cursor as read_u32,
};
use crate::framing::{ClientFrame, ServerFrame, upstream_edge::write_server_frame};

pub(super) const SMSG_FORCE_RUN_SPEED_CHANGE_OPCODE: u16 = 0x00E2;
const CMSG_FORCE_RUN_SPEED_CHANGE_ACK_OPCODE: u32 = 0x00E3;

pub(super) const SMSG_TIME_SYNC_REQ_OPCODE: u16 = 0x0390;
const CMSG_TIME_SYNC_RESP_OPCODE: u32 = 0x0391;
pub(super) const BOT_VISUAL_ECHO_WINDOW: Duration = Duration::from_millis(1250);
const ECHO_POSITION_DRIFT_YARDS: f32 = 2.0;
const ECHO_DRIFT_SPEED_YARDS_PER_SECOND: f32 = 7.0;
const MAX_ECHO_POSITION_DRIFT_YARDS: f32 = 9.0;
const MAX_ECHO_ORIENTATION_DRIFT_RADIANS: f32 = 0.35;

#[derive(Debug)]
pub(super) struct MovementClock {
    client_time_ms: u32,
    anchored_at: Instant,
    last_generated: Option<u32>,
}

const MAX_ABS_COORDINATE: f32 = 200_000.0;
const MAX_BOT_PACKET_DELTA_YARDS: f32 = 40.0;
const MAX_PLAYER_PACKET_DELTA_YARDS: f32 = 250.0;
const BOT_DELTA_ALLOWANCE_YARDS: f32 = 2.0;
const BOT_DELTA_ALLOWANCE_PER_MS: f32 = 0.01;
const MIN_VISUAL_MOVE_MS: u32 = 40;
const MAX_VISUAL_MOVE_MS: u32 = 10_000;

pub(super) fn validate_movement_position(
    from: Option<WorldPosition>,
    to: Vec3,
    elapsed_ms: u32,
    source: MovementSource,
) -> Result<(), &'static str> {
    if !to.is_finite()
        || [to.x, to.y, to.z]
            .into_iter()
            .any(|v| v.abs() > MAX_ABS_COORDINATE)
    {
        return Err("position is non-finite or outside world coordinate bounds");
    }
    let Some(from) = from else {
        return Ok(());
    };
    if !from.point.is_finite()
        || [from.point.x, from.point.y, from.point.z]
            .into_iter()
            .any(|v| v.abs() > MAX_ABS_COORDINATE)
    {
        return Err("canonical position is invalid");
    }
    let max_delta = match source {
        MovementSource::Bot => {
            if elapsed_ms == 0 {
                12.0
            } else {
                (BOT_DELTA_ALLOWANCE_YARDS + BOT_DELTA_ALLOWANCE_PER_MS * elapsed_ms as f32)
                    .min(MAX_BOT_PACKET_DELTA_YARDS)
            }
        }
        MovementSource::Player => MAX_PLAYER_PACKET_DELTA_YARDS,
    };
    if from.point.distance(to) > max_delta {
        return Err("movement exceeds the allowed packet displacement");
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(super) enum MovementSource {
    Bot,
    Player,
}

#[derive(Clone, Copy)]
struct VisualSegment {
    start: WorldPosition,
    destination: WorldPosition,
    started_at: u32,
    duration_ms: u32,
}

#[derive(Default)]
pub(super) struct ServerMotionProjection {
    next_spline_id: u32,
    last: Option<(EntityId, WorldPosition, u32)>,
    visual: Option<VisualSegment>,
}

impl ServerMotionProjection {
    pub(super) fn reset_to(&mut self, mover: EntityId, position: WorldPosition, client_time: u32) {
        self.last = Some((mover, position, client_time));
        self.visual = None;
    }

    pub(super) fn project(
        &mut self,
        mover: EntityId,
        position: WorldPosition,
        client_time: u32,
        moving: bool,
    ) -> Option<ServerFrame> {
        let (last_mover, previous, previous_time) = self.last?;
        if last_mover != mover || !position.point.is_finite() || !position.orientation.is_finite() {
            self.reset_to(mover, position, client_time);
            return None;
        }
        let elapsed = client_time.wrapping_sub(previous_time);
        let distance = previous.point.distance(position.point);
        let facing_delta = signed_angle_delta(previous.orientation, position.orientation);
        if !moving {
            self.last = Some((mover, position, client_time));
            self.visual = None;
            if distance <= 0.001 && facing_delta.abs() <= 0.01 {
                return None;
            }
            return Some(self.move_frame(previous, position, 1));
        }
        if elapsed < MIN_VISUAL_MOVE_MS {
            return None;
        }
        if distance <= 0.001 && facing_delta.abs() <= 0.01 {
            self.last = Some((mover, position, client_time));
            return None;
        }
        let start = self
            .visual
            .and_then(|segment| project_segment(segment, client_time))
            .unwrap_or(previous);
        self.last = Some((mover, position, client_time));
        let duration = elapsed
            .clamp(MIN_VISUAL_MOVE_MS, MAX_VISUAL_MOVE_MS)
            .saturating_add(50)
            .min(MAX_VISUAL_MOVE_MS);
        self.visual = Some(VisualSegment {
            start,
            destination: position,
            started_at: client_time,
            duration_ms: duration,
        });
        Some(self.move_frame(start, position, duration))
    }

    fn move_frame(
        &mut self,
        start: WorldPosition,
        destination: WorldPosition,
        duration_ms: u32,
    ) -> ServerFrame {
        self.next_spline_id = self.next_spline_id.wrapping_add(1).max(1);
        let mut body = Vec::with_capacity(48);
        push_packed_guid(
            &mut body,
            self.last.map(|(mover, _, _)| mover).unwrap_or(EntityId(0)),
        );
        body.push(0);
        push_position(&mut body, start.point);
        body.extend_from_slice(&self.next_spline_id.to_le_bytes());
        if signed_angle_delta(start.orientation, destination.orientation).abs() > 0.01 {
            body.push(4);
            body.extend_from_slice(&destination.orientation.to_le_bytes());
        } else {
            body.push(0);
        }
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.extend_from_slice(&duration_ms.to_le_bytes());
        body.extend_from_slice(&1_u32.to_le_bytes());
        push_position(&mut body, destination.point);
        ServerFrame {
            opcode: 0x00DD,
            body,
        }
    }

    pub(super) fn stop(&mut self, mover: EntityId, position: WorldPosition) -> Option<ServerFrame> {
        self.visual = None;
        self.last = Some((
            mover,
            position,
            self.last.map(|(_, _, time)| time).unwrap_or_default(),
        ));
        self.next_spline_id = self.next_spline_id.wrapping_add(1).max(1);
        let mut body = Vec::with_capacity(32);
        push_packed_guid(&mut body, mover);
        body.push(0);
        push_position(&mut body, position.point);
        body.extend_from_slice(&self.next_spline_id.to_le_bytes());
        body.push(1);
        Some(ServerFrame {
            opcode: 0x00DD,
            body,
        })
    }
}

fn project_segment(segment: VisualSegment, client_time: u32) -> Option<WorldPosition> {
    let elapsed = client_time.wrapping_sub(segment.started_at);
    if elapsed > i32::MAX as u32 {
        return Some(segment.destination);
    }
    let t = (elapsed as f32 / segment.duration_ms.max(1) as f32).clamp(0.0, 1.0);
    let start = segment.start;
    let end = segment.destination;
    let delta = signed_angle_delta(start.orientation, end.orientation);
    Some(WorldPosition {
        map: end.map,
        point: Vec3::new(
            start.point.x + (end.point.x - start.point.x) * t,
            start.point.y + (end.point.y - start.point.y) * t,
            start.point.z + (end.point.z - start.point.z) * t,
        ),
        orientation: (start.orientation + delta * t).rem_euclid(std::f32::consts::TAU),
    })
}

fn signed_angle_delta(from: f32, to: f32) -> f32 {
    (to - from + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}
fn push_position(body: &mut Vec<u8>, point: Vec3) {
    body.extend_from_slice(&point.x.to_le_bytes());
    body.extend_from_slice(&point.y.to_le_bytes());
    body.extend_from_slice(&point.z.to_le_bytes());
}

impl Default for MovementClock {
    fn default() -> Self {
        Self {
            client_time_ms: 1,
            anchored_at: Instant::now(),
            last_generated: None,
        }
    }
}

impl MovementClock {
    pub(super) fn current_timestamp(&self) -> u32 {
        let current = self
            .client_time_ms
            .wrapping_add(self.anchored_at.elapsed().as_millis() as u32);
        match self.last_generated {
            Some(last) if !timestamp_is_after(current, last) => last,
            _ => current,
        }
    }

    pub(super) fn observe(&mut self, client_time_ms: u32) {
        let observed = match self.last_generated {
            Some(last) if !timestamp_is_after_or_equal(client_time_ms, last) => last,
            _ => client_time_ms,
        };
        self.client_time_ms = observed;
        self.anchored_at = Instant::now();
        if self
            .last_generated
            .is_none_or(|last| timestamp_is_after(observed, last))
        {
            self.last_generated = Some(observed);
        }
    }

    pub(super) fn next_timestamp(&mut self) -> u32 {
        let current = self.current_timestamp();
        let next = match self.last_generated {
            Some(last) if !timestamp_is_after(current, last) => last.wrapping_add(1),
            _ => current,
        };
        self.last_generated = Some(next);
        next
    }
}

pub(super) fn parse_server_near_teleport(body: &[u8]) -> Option<(EntityId, u32, Vec3, f32)> {
    let (&mask, mut rest) = body.split_first()?;
    let mut guid = 0_u64;
    for index in 0..8 {
        if mask & (1 << index) != 0 {
            let (&byte, tail) = rest.split_first()?;
            rest = tail;
            guid |= u64::from(byte) << (index * 8);
        }
    }
    if guid == 0 {
        return None;
    }

    // Server near-teleport requests contain packed mover GUID, order counter,
    // then MovementInfo. The client ACK contains the GUID, flags, and its own
    // movement timestamp.
    let movement = rest.get(4..)?;
    let flags = u32::from_le_bytes(movement.get(..4)?.try_into().ok()?);
    let position = movement.get(10..26)?;
    let x = f32::from_le_bytes(position.get(..4)?.try_into().ok()?);
    let y = f32::from_le_bytes(position.get(4..8)?.try_into().ok()?);
    let z = f32::from_le_bytes(position.get(8..12)?.try_into().ok()?);
    let orientation = f32::from_le_bytes(position.get(12..16)?.try_into().ok()?);
    let point = Vec3::new(x, y, z);
    (point.is_finite() && orientation.is_finite()).then_some((
        EntityId(guid),
        flags,
        point,
        orientation,
    ))
}

fn timestamp_is_after(candidate: u32, previous: u32) -> bool {
    (candidate.wrapping_sub(previous) as i32) > 0
}

fn timestamp_is_after_or_equal(candidate: u32, previous: u32) -> bool {
    candidate == previous || timestamp_is_after(candidate, previous)
}

fn movement_pose_is_finite(position: WorldPosition) -> bool {
    position.point.is_finite() && position.orientation.is_finite()
}

pub(super) fn parse_time_sync_response(body: &[u8]) -> Option<(u32, u32)> {
    let counter = u32::from_le_bytes(body.get(..4)?.try_into().ok()?);
    let client_time_ms = u32::from_le_bytes(body.get(4..8)?.try_into().ok()?);
    Some((counter, client_time_ms))
}

pub(super) fn parse_time_sync_request(body: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(body.get(..4)?.try_into().ok()?))
}

pub(super) fn encode_time_sync_response(counter: u32, client_time_ms: u32) -> ClientFrame {
    let mut body = Vec::with_capacity(8);
    body.extend_from_slice(&counter.to_le_bytes());
    body.extend_from_slice(&client_time_ms.to_le_bytes());
    ClientFrame {
        opcode: CMSG_TIME_SYNC_RESP_OPCODE,
        body,
    }
}

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LootResponse {
    pub guid: u64,
    pub loot_type: u8,
    pub error: Option<u8>,
    pub gold: u32,
    pub slots: Vec<u8>,
}

pub(super) fn parse_loot_response(body: &[u8]) -> Option<LootResponse> {
    // Wrath SMSG_LOOT_RESPONSE: guid:u64, loot_type:u8, gold:u32,
    // item_count:u8, followed by 22-byte item records beginning with slot:u8.
    let guid = u64::from_le_bytes(body.get(0..8)?.try_into().ok()?);
    let loot_type = *body.get(8)?;
    if loot_type == 0 {
        // Rejections use the short form: guid:u64, loot_type:u8, error:u8.
        return Some(LootResponse {
            guid,
            loot_type,
            error: body.get(9).copied(),
            gold: 0,
            slots: Vec::new(),
        });
    }
    let gold = u32::from_le_bytes(body.get(9..13)?.try_into().ok()?);
    let count = usize::from(*body.get(13)?);
    if count > 18 {
        return None;
    }
    let mut cursor = 14usize;
    let mut slots = Vec::with_capacity(count);
    for _ in 0..count {
        let slot = *body.get(cursor)?;
        body.get(cursor..cursor + 22)?;
        slots.push(slot);
        cursor += 22;
    }
    Some(LootResponse {
        guid,
        loot_type,
        error: None,
        gold,
        slots,
    })
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
    movement_clock: &mut MovementClock,
) -> std::result::Result<Option<(ClientFrame, Option<(WorldPosition, bool, u32, u32)>)>, String> {
    const CMSG_USE_ITEM: u32 = 0x00AB;
    const CMSG_CAST_SPELL: u32 = 0x012F;
    const CMSG_GAMEOBJ_USE: u32 = 0x00B1;
    const MSG_MOVE_STOP: u32 = 0x00B7;
    const MSG_MOVE_START_FORWARD: u32 = 0x00B5;
    const MSG_MOVE_SET_FACING: u32 = 0x00DA;
    const MSG_MOVE_HEARTBEAT: u32 = 0x00EE;
    const CMSG_ATTACKSWING: u32 = 0x0141;
    const CMSG_AUTOEQUIP_ITEM_SLOT: u32 = 0x010F;
    const CMSG_PET_ACTION: u32 = 0x0175;
    const CMSG_PET_SPELL_AUTOCAST: u32 = 0x02F3;
    const CMSG_LOOT: u32 = 0x015D;
    const CMSG_QUESTGIVER_HELLO: u32 = 0x0184;
    const CMSG_QUESTGIVER_ACCEPT_QUEST: u32 = 0x0189;
    const CMSG_QUESTGIVER_COMPLETE_QUEST: u32 = 0x018A;
    const CMSG_QUESTGIVER_REQUEST_REWARD: u32 = 0x018C;
    const CMSG_QUESTGIVER_CHOOSE_REWARD: u32 = 0x018E;
    const CMSG_QUESTGIVER_STATUS_MULTIPLE_QUERY: u32 = 0x0417;
    const CMSG_QUEST_QUERY: u32 = 0x005C;
    const CMSG_ITEM_QUERY_SINGLE: u32 = 0x0056;
    const CMSG_LIST_INVENTORY: u32 = 0x019E;
    const CMSG_BUY_ITEM: u32 = 0x01A2;
    const CMSG_SELL_ITEM: u32 = 0x01A0;
    const CMSG_REPAIR_ITEM: u32 = 0x02A8;
    const CMSG_TRAINER_LIST: u32 = 0x01B0;
    const CMSG_TRAINER_BUY_SPELL: u32 = 0x01B2;
    const CMSG_GET_MAIL_LIST: u32 = 0x023A;
    const CMSG_MAIL_TAKE_MONEY: u32 = 0x0245;
    const CMSG_MAIL_TAKE_ITEM: u32 = 0x0246;
    const CMSG_SET_AMMO: u32 = 0x0268;
    const CMSG_BANKER_ACTIVATE: u32 = 0x01B7;
    const CMSG_AUTOBANK_ITEM: u32 = 0x0283;
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
        GameplayCommand::QueryItem { item } => {
            let mut body = item.to_le_bytes().to_vec();
            body.extend_from_slice(&0u64.to_le_bytes());
            Ok(Some((
                ClientFrame {
                    opcode: CMSG_ITEM_QUERY_SINGLE,
                    body,
                },
                None,
            )))
        }
        GameplayCommand::VendorList { vendor } => Ok(Some((
            ClientFrame {
                opcode: CMSG_LIST_INVENTORY,
                body: raw_guid_body(vendor),
            },
            None,
        ))),
        GameplayCommand::TrainerList { trainer } => Ok(Some((
            ClientFrame {
                opcode: CMSG_TRAINER_LIST,
                body: raw_guid_body(trainer),
            },
            None,
        ))),
        GameplayCommand::TrainerBuy { trainer, spell } => {
            let mut body = raw_guid_body(trainer);
            body.extend_from_slice(&spell.to_le_bytes());
            Ok(Some((
                ClientFrame {
                    opcode: CMSG_TRAINER_BUY_SPELL,
                    body,
                },
                None,
            )))
        }
        GameplayCommand::VendorBuy {
            vendor,
            item,
            slot,
            count,
        } => {
            let mut body = raw_guid_body(vendor);
            body.extend_from_slice(&item.to_le_bytes());
            body.extend_from_slice(&slot.to_le_bytes());
            body.extend_from_slice(&count.to_le_bytes());
            body.push(0); // inventory bag
            Ok(Some((
                ClientFrame {
                    opcode: CMSG_BUY_ITEM,
                    body,
                },
                None,
            )))
        }
        GameplayCommand::SetAmmo { item } => Ok(Some((
            ClientFrame {
                opcode: CMSG_SET_AMMO,
                body: item.to_le_bytes().to_vec(),
            },
            None,
        ))),
        GameplayCommand::VendorSell {
            vendor,
            item_guid,
            count,
            ..
        } => {
            let mut body = raw_guid_body(vendor);
            body.extend_from_slice(&item_guid.0.to_le_bytes());
            body.extend_from_slice(&count.to_le_bytes());
            Ok(Some((
                ClientFrame {
                    opcode: CMSG_SELL_ITEM,
                    body,
                },
                None,
            )))
        }
        GameplayCommand::RepairEquipment { vendor } => {
            let mut body = raw_guid_body(vendor);
            body.extend_from_slice(&0_u64.to_le_bytes());
            body.push(0);
            Ok(Some((
                ClientFrame {
                    opcode: CMSG_REPAIR_ITEM,
                    body,
                },
                None,
            )))
        }
        GameplayCommand::BankActivate { banker } => Ok(Some((
            ClientFrame {
                opcode: CMSG_BANKER_ACTIVATE,
                body: banker.0.to_le_bytes().to_vec(),
            },
            None,
        ))),
        GameplayCommand::BankDeposit { backpack_slot, .. } => Ok(Some((
            ClientFrame {
                opcode: CMSG_AUTOBANK_ITEM,
                body: vec![0xff, backpack_slot], // backpack bag sentinel, then absolute slot
            },
            None,
        ))),
        GameplayCommand::MailboxList { mailbox } => Ok(Some((
            ClientFrame {
                opcode: CMSG_GET_MAIL_LIST,
                body: mailbox.0.to_le_bytes().to_vec(),
            },
            None,
        ))),
        GameplayCommand::MailTake {
            mailbox,
            mail_id,
            target,
            ..
        } => {
            let mut body = Vec::with_capacity(16);
            body.extend_from_slice(&mailbox.0.to_le_bytes());
            body.extend_from_slice(&mail_id.to_le_bytes());
            let opcode = match target {
                wow_domain::MailTakeTarget::Money => CMSG_MAIL_TAKE_MONEY,
                wow_domain::MailTakeTarget::Attachment { low_guid } => {
                    body.extend_from_slice(&low_guid.to_le_bytes());
                    CMSG_MAIL_TAKE_ITEM
                }
            };
            Ok(Some((ClientFrame { opcode, body }, None)))
        }
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
        GameplayCommand::CastOnItem { spell, item_guid } => {
            let mut body = Vec::with_capacity(20);
            body.push(0); // client cast sequence
            body.extend_from_slice(&spell.to_le_bytes());
            body.push(0); // cast flags
            body.extend_from_slice(&0x0000_0010_u32.to_le_bytes()); // TARGET_FLAG_ITEM
            push_packed_guid(&mut body, item_guid);
            Ok(Some((
                ClientFrame {
                    opcode: CMSG_CAST_SPELL,
                    body,
                },
                None,
            )))
        }
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
        GameplayCommand::UseItemOnItem {
            item: _,
            item_guid,
            backpack_slot,
            spell,
            target_item_guid,
        } => {
            let mut body = Vec::with_capacity(32);
            body.extend_from_slice(&[0xff, backpack_slot, 0]);
            body.extend_from_slice(&spell.to_le_bytes());
            append_raw_guid(&mut body, item_guid);
            body.extend_from_slice(&0_u32.to_le_bytes()); // glyph index
            body.push(0); // cast flags
            body.extend_from_slice(&0x0000_0010_u32.to_le_bytes()); // TARGET_FLAG_ITEM
            push_packed_guid(&mut body, target_item_guid);
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
        GameplayCommand::CancelMount => Ok(Some((
            ClientFrame {
                opcode: 0x0375,
                body: Vec::new(),
            },
            None,
        ))),
        GameplayCommand::CancelAura { spell } => Ok(Some((
            ClientFrame {
                opcode: 0x0136,
                body: spell.to_le_bytes().to_vec(),
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
        GameplayCommand::SummonPet { spell, .. } => Ok(Some((
            ClientFrame {
                opcode: 0x012E,
                body: encode_cast_spell(spell, None, 0x0000_0002),
            },
            None,
        ))),
        GameplayCommand::EquipItem {
            item_guid,
            destination_slot,
        } => {
            let mut body = item_guid.0.to_le_bytes().to_vec();
            body.push(destination_slot);
            Ok(Some((
                ClientFrame {
                    opcode: CMSG_AUTOEQUIP_ITEM_SLOT,
                    body,
                },
                None,
            )))
        }
        GameplayCommand::PetSetReaction { pet, reaction } => {
            let body = encode_pet_action(pet, u32::from(reaction), 0x06, EntityId(0));
            Ok(Some((
                ClientFrame {
                    opcode: CMSG_PET_ACTION,
                    body,
                },
                None,
            )))
        }
        GameplayCommand::PetAttack { pet, target } => {
            let body = encode_pet_action(pet, 2, 0x07, target);
            Ok(Some((
                ClientFrame {
                    opcode: CMSG_PET_ACTION,
                    body,
                },
                None,
            )))
        }
        GameplayCommand::PetSetAutocast {
            pet,
            spell,
            enabled,
        } => {
            let mut body = pet.0.to_le_bytes().to_vec();
            body.extend_from_slice(&spell.to_le_bytes());
            body.push(u8::from(enabled));
            Ok(Some((
                ClientFrame {
                    opcode: CMSG_PET_SPELL_AUTOCAST,
                    body,
                },
                None,
            )))
        }
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
            if !movement_pose_is_finite(position) {
                return Err("facing requested from an invalid canonical position".into());
            }
            if !orientation.is_finite() {
                return Err("facing orientation is invalid".into());
            }
            position.orientation = Radians::normalized(orientation)
                .expect("finite facing orientation was checked above")
                .0;
            let movement_time = movement_clock.next_timestamp();
            let flags = base_movement_flags & !0x0000_0001_u32;
            let body = encode_simple_movement(
                guid,
                flags,
                movement_time,
                position.point,
                position.orientation,
            );
            Ok(Some((
                ClientFrame {
                    opcode: MSG_MOVE_SET_FACING,
                    body,
                },
                Some((position, false, flags, movement_time)),
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
            if !movement_pose_is_finite(position) {
                return Err("movement requested from an invalid canonical position".into());
            }
            if !destination.is_finite() {
                return Err("movement destination is invalid".into());
            }
            let dx = destination.x - position.point.x;
            let dy = destination.y - position.point.y;
            if dx != 0.0 || dy != 0.0 {
                position.orientation = dy.atan2(dx);
            }
            position.point = destination;
            let movement_time = movement_clock.next_timestamp();
            // Preserve server-authoritative capabilities such as flying/disable-gravity,
            // while setting forward movement for this step.
            let flags = base_movement_flags | 0x0000_0001_u32;
            let body = encode_simple_movement(
                guid,
                flags,
                movement_time,
                position.point,
                position.orientation,
            );
            Ok(Some((
                ClientFrame {
                    opcode: if base_movement_flags & 0x0000_0001 == 0 {
                        MSG_MOVE_START_FORWARD
                    } else {
                        MSG_MOVE_HEARTBEAT
                    },
                    body,
                },
                Some((position, true, flags, movement_time)),
            )))
        }
        GameplayCommand::StopMovement => {
            let guid = player_guid.ok_or_else(|| {
                "stop movement requested before player GUID is authoritative".to_owned()
            })?;
            let position = current.ok_or_else(|| {
                "stop movement requested before canonical position is known".to_owned()
            })?;
            if !movement_pose_is_finite(position) {
                return Err("stop movement requested from an invalid canonical position".into());
            }
            let movement_time = movement_clock.next_timestamp();
            let flags = base_movement_flags & !0x0000_0001_u32;
            let body = encode_simple_movement(
                guid,
                flags,
                movement_time,
                position.point,
                position.orientation,
            );
            Ok(Some((
                ClientFrame {
                    opcode: MSG_MOVE_STOP,
                    body,
                },
                Some((position, false, flags, movement_time)),
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

fn encode_pet_action(pet: EntityId, action: u32, action_type: u32, target: EntityId) -> Vec<u8> {
    let mut body = Vec::with_capacity(20);
    append_raw_guid(&mut body, pet);
    body.extend_from_slice(&((action & 0x00ff_ffff) | ((action_type & 0xff) << 24)).to_le_bytes());
    append_raw_guid(&mut body, target);
    body
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

pub(super) fn parse_force_run_speed_change(body: &[u8]) -> Option<(EntityId, u32, f32)> {
    let (&mask, mut rest) = body.split_first()?;
    let mut guid = 0_u64;
    for index in 0..8 {
        if mask & (1 << index) != 0 {
            let (&byte, tail) = rest.split_first()?;
            rest = tail;
            guid |= u64::from(byte) << (index * 8);
        }
    }
    let move_event = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?);
    let rest = rest.get(5..)?;
    let speed = f32::from_le_bytes(rest.get(..4)?.try_into().ok()?);
    (guid != 0 && rest.len() == 4 && speed.is_finite() && (0.1..=100.0).contains(&speed))
        .then_some((EntityId(guid), move_event, speed))
}

pub(super) fn encode_force_run_speed_change_ack(
    guid: EntityId,
    move_event: u32,
    speed: f32,
    client_time: u32,
    position: WorldPosition,
) -> ClientFrame {
    let mut body = Vec::with_capacity(42);
    push_packed_guid(&mut body, guid);
    body.extend_from_slice(&move_event.to_le_bytes());
    // MovementInfo with no movement flags, followed by the confirmed speed.
    body.extend_from_slice(&0_u32.to_le_bytes());
    body.extend_from_slice(&0_u16.to_le_bytes());
    body.extend_from_slice(&client_time.to_le_bytes());
    body.extend_from_slice(&position.point.x.to_le_bytes());
    body.extend_from_slice(&position.point.y.to_le_bytes());
    body.extend_from_slice(&position.point.z.to_le_bytes());
    body.extend_from_slice(&position.orientation.to_le_bytes());
    body.extend_from_slice(&0.0_f32.to_le_bytes());
    body.extend_from_slice(&speed.to_le_bytes());
    ClientFrame {
        opcode: CMSG_FORCE_RUN_SPEED_CHANGE_ACK_OPCODE,
        body,
    }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MovementCorrectionExpectation {
    pub ack_opcode: u32,
    pub mover: EntityId,
    pub counter: u32,
    pub near_teleport_flags: Option<u32>,
}

fn decode_movement_info_payload(body: &[u8], cursor: &mut usize) -> Option<(u32, u32, Vec3, f32)> {
    let flags = read_u32(body, cursor)?;
    let extra_flags = read_u16(body, cursor)?;
    let client_time = read_u32(body, cursor)?;
    let point = Vec3::new(
        read_f32(body, cursor)?,
        read_f32(body, cursor)?,
        read_f32(body, cursor)?,
    );
    let orientation = read_f32(body, cursor)?;

    if flags & 0x0000_0200 != 0 {
        read_packed_guid(body, cursor)?;
        for _ in 0..4 {
            read_f32(body, cursor)?;
        }
        read_u32(body, cursor)?;
        body.get(*cursor)?;
        *cursor += 1;
        if extra_flags & 0x0400 != 0 {
            read_u32(body, cursor)?;
        }
    }
    if flags & (0x0020_0000 | 0x0200_0000) != 0 || extra_flags & 0x0020 != 0 {
        read_f32(body, cursor)?;
    }
    read_u32(body, cursor)?;
    if flags & 0x0000_1000 != 0 {
        for _ in 0..4 {
            read_f32(body, cursor)?;
        }
    }
    if flags & 0x0400_0000 != 0 {
        read_f32(body, cursor)?;
    }
    Some((flags, client_time, point, orientation))
}

/// Decode one complete WotLK MovementInfo body. Conditional fields follow the
/// same flags as AzerothCore's WorldSession::ReadMovementInfo.
pub(super) fn decode_simple_movement(
    opcode: u32,
    body: &[u8],
) -> Option<(EntityId, u32, u32, Vec3, f32)> {
    let mut cursor = 0usize;
    let guid = read_packed_guid(body, &mut cursor)?;
    let (flags, client_time, point, orientation) = decode_movement_info_payload(body, &mut cursor)?;
    if cursor != body.len() || guid == 0 || !point.is_finite() || !orientation.is_finite() {
        return None;
    }
    debug_assert!(is_player_movement_opcode(opcode));
    Some((EntityId(guid), flags, client_time, point, orientation))
}

pub(super) fn decode_near_teleport_ack(body: &[u8]) -> Option<(EntityId, u32, u32)> {
    let mut cursor = 0usize;
    let guid = read_packed_guid(body, &mut cursor)?;
    let flags = read_u32(body, &mut cursor)?;
    let client_time = read_u32(body, &mut cursor)?;
    (guid != 0 && cursor == body.len()).then_some((EntityId(guid), flags, client_time))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CorrectionAckFormat {
    NearTeleport,
    NoTail,
    FiniteFloatTail,
    OpaqueFourByteTail,
}

#[derive(Clone, Copy)]
struct CorrectionAckOpcode {
    request: u32,
    ack: u32,
    format: CorrectionAckFormat,
}

const CORRECTION_ACK_OPCODES: &[CorrectionAckOpcode] = &[
    CorrectionAckOpcode {
        request: 0x00C7,
        ack: 0x00C7,
        format: CorrectionAckFormat::NearTeleport,
    },
    CorrectionAckOpcode {
        request: 0x00EF,
        ack: 0x00F0,
        format: CorrectionAckFormat::NoTail,
    },
    CorrectionAckOpcode {
        request: 0x00E8,
        ack: 0x00E9,
        format: CorrectionAckFormat::NoTail,
    },
    CorrectionAckOpcode {
        request: 0x00EA,
        ack: 0x00EB,
        format: CorrectionAckFormat::NoTail,
    },
    CorrectionAckOpcode {
        request: 0x00E2,
        ack: 0x00E3,
        format: CorrectionAckFormat::FiniteFloatTail,
    },
    CorrectionAckOpcode {
        request: 0x00E4,
        ack: 0x00E5,
        format: CorrectionAckFormat::FiniteFloatTail,
    },
    CorrectionAckOpcode {
        request: 0x00E6,
        ack: 0x00E7,
        format: CorrectionAckFormat::FiniteFloatTail,
    },
    CorrectionAckOpcode {
        request: 0x00F2,
        ack: 0x02CF,
        format: CorrectionAckFormat::OpaqueFourByteTail,
    },
    CorrectionAckOpcode {
        request: 0x00F3,
        ack: 0x02CF,
        format: CorrectionAckFormat::OpaqueFourByteTail,
    },
    CorrectionAckOpcode {
        request: 0x00F4,
        ack: 0x00F6,
        format: CorrectionAckFormat::OpaqueFourByteTail,
    },
    CorrectionAckOpcode {
        request: 0x00F5,
        ack: 0x00F6,
        format: CorrectionAckFormat::OpaqueFourByteTail,
    },
    CorrectionAckOpcode {
        request: 0x00DE,
        ack: 0x02D0,
        format: CorrectionAckFormat::OpaqueFourByteTail,
    },
    CorrectionAckOpcode {
        request: 0x00DF,
        ack: 0x02D0,
        format: CorrectionAckFormat::OpaqueFourByteTail,
    },
    CorrectionAckOpcode {
        request: 0x02DA,
        ack: 0x02DB,
        format: CorrectionAckFormat::FiniteFloatTail,
    },
    CorrectionAckOpcode {
        request: 0x02DC,
        ack: 0x02DD,
        format: CorrectionAckFormat::FiniteFloatTail,
    },
    CorrectionAckOpcode {
        request: 0x02DE,
        ack: 0x02DF,
        format: CorrectionAckFormat::FiniteFloatTail,
    },
    CorrectionAckOpcode {
        request: 0x0343,
        ack: 0x0345,
        format: CorrectionAckFormat::OpaqueFourByteTail,
    },
    CorrectionAckOpcode {
        request: 0x0344,
        ack: 0x0345,
        format: CorrectionAckFormat::OpaqueFourByteTail,
    },
    CorrectionAckOpcode {
        request: 0x033E,
        ack: 0x0340,
        format: CorrectionAckFormat::FiniteFloatTail,
    },
    CorrectionAckOpcode {
        request: 0x033F,
        ack: 0x0340,
        format: CorrectionAckFormat::FiniteFloatTail,
    },
    CorrectionAckOpcode {
        request: 0x0381,
        ack: 0x0382,
        format: CorrectionAckFormat::FiniteFloatTail,
    },
    CorrectionAckOpcode {
        request: 0x0383,
        ack: 0x0384,
        format: CorrectionAckFormat::FiniteFloatTail,
    },
    CorrectionAckOpcode {
        request: 0x045C,
        ack: 0x045D,
        format: CorrectionAckFormat::FiniteFloatTail,
    },
    CorrectionAckOpcode {
        request: 0x04CE,
        ack: 0x04CF,
        format: CorrectionAckFormat::FiniteFloatTail,
    },
    CorrectionAckOpcode {
        request: 0x04D0,
        ack: 0x04D1,
        format: CorrectionAckFormat::FiniteFloatTail,
    },
    CorrectionAckOpcode {
        request: 0x0516,
        ack: 0x0517,
        format: CorrectionAckFormat::FiniteFloatTail,
    },
];

fn correction_ack_opcode(request_opcode: u32) -> Option<u32> {
    CORRECTION_ACK_OPCODES
        .iter()
        .find(|entry| entry.request == request_opcode)
        .map(|entry| entry.ack)
}

fn correction_ack_format(ack_opcode: u32) -> Option<CorrectionAckFormat> {
    CORRECTION_ACK_OPCODES
        .iter()
        .find(|entry| entry.ack == ack_opcode)
        .map(|entry| entry.format)
}

pub(super) fn is_movement_correction_ack_opcode(opcode: u32) -> bool {
    correction_ack_format(opcode).is_some()
}

pub(super) fn movement_correction_expectation(
    request_opcode: u32,
    body: &[u8],
) -> Option<MovementCorrectionExpectation> {
    let ack_opcode = correction_ack_opcode(request_opcode)?;
    let mut cursor = 0usize;
    let mover = EntityId(read_packed_guid(body, &mut cursor)?);
    if mover == EntityId(0) {
        return None;
    }
    let counter = read_u32(body, &mut cursor)?;
    let near_teleport_flags = if request_opcode == 0x00C7 {
        let (flags, _, _, _) = decode_movement_info_payload(body, &mut cursor)?;
        if cursor != body.len() {
            return None;
        }
        Some(flags)
    } else {
        None
    };
    Some(MovementCorrectionExpectation {
        ack_opcode,
        mover,
        counter,
        near_teleport_flags,
    })
}

pub(super) fn validate_movement_correction_ack(
    opcode: u32,
    body: &[u8],
    expectation: MovementCorrectionExpectation,
) -> Result<EntityId, &'static str> {
    if opcode != expectation.ack_opcode {
        return Err("correction acknowledgement opcode does not match pending request");
    }
    let format = correction_ack_format(opcode)
        .ok_or("unsupported movement correction acknowledgement opcode")?;
    if format == CorrectionAckFormat::NearTeleport {
        let (mover, flags, _) = decode_near_teleport_ack(body)
            .ok_or("near-teleport acknowledgement has malformed layout")?;
        if mover != expectation.mover || expectation.near_teleport_flags != Some(flags) {
            return Err("near-teleport acknowledgement mover or flags do not match request");
        }
        return Ok(mover);
    }

    let mut cursor = 0usize;
    let mover = EntityId(
        read_packed_guid(body, &mut cursor)
            .ok_or("correction acknowledgement has malformed mover GUID")?,
    );
    let counter = read_u32(body, &mut cursor)
        .ok_or("correction acknowledgement is missing request counter")?;
    if mover != expectation.mover || counter != expectation.counter {
        return Err("correction acknowledgement mover or counter does not match request");
    }
    decode_movement_info_payload(body, &mut cursor)
        .ok_or("correction acknowledgement has malformed MovementInfo")?;
    let tail_len = body.len().saturating_sub(cursor);
    let expected_tail = match format {
        CorrectionAckFormat::NoTail => 0,
        CorrectionAckFormat::FiniteFloatTail | CorrectionAckFormat::OpaqueFourByteTail => 4,
        CorrectionAckFormat::NearTeleport => unreachable!("handled above"),
    };
    if tail_len != expected_tail {
        return Err("correction acknowledgement has invalid trailing fields");
    }
    if format == CorrectionAckFormat::FiniteFloatTail {
        let value = f32::from_le_bytes(
            body[cursor..]
                .try_into()
                .map_err(|_| "invalid correction value")?,
        );
        if !value.is_finite() {
            return Err("correction acknowledgement value is not finite");
        }
    }
    Ok(mover)
}

pub(super) fn movement_matches_bot_visual(
    visual: WorldPosition,
    visual_mover: EntityId,
    bot_opcode: u32,
    client_mover: EntityId,
    point: Vec3,
    orientation: f32,
    client_opcode: u32,
    visual_age: Duration,
) -> bool {
    // Increase the position allowance as the client trails the bot visual.
    // The cap covers 7 yards per second during the 1.25-second echo window.
    let max_position_drift = (ECHO_POSITION_DRIFT_YARDS
        + ECHO_DRIFT_SPEED_YARDS_PER_SECOND * visual_age.as_secs_f32())
    .min(MAX_ECHO_POSITION_DRIFT_YARDS);
    if visual_mover == EntityId(0)
        || visual_mover != client_mover
        || !point.is_finite()
        || !orientation.is_finite()
    {
        return false;
    }
    let Some(delta) = Radians::normalized(visual.orientation - orientation).map(|value| value.0)
    else {
        return false;
    };
    let angular = delta.min(std::f32::consts::TAU - delta);
    point.distance(visual.point) <= max_position_drift
        && angular <= MAX_ECHO_ORIENTATION_DRIFT_RADIANS
        && (client_opcode == bot_opcode
            || client_opcode == 0x00EE
            || (is_explicit_player_movement_intent(client_opcode)
                && is_explicit_player_movement_intent(bot_opcode)))
}

pub(super) fn should_suppress_bot_echo(matching_echo: bool) -> bool {
    matching_echo
}

/// Check whether a client movement packet is valid physical takeover input.
/// Passive feedback and correction acknowledgements do not pass this check.
pub(super) fn validate_player_takeover(
    opcode: u32,
    mover: EntityId,
    flags: u32,
    expected_mover: Option<EntityId>,
) -> Result<(), &'static str> {
    if !is_explicit_player_movement_intent(opcode) {
        return Err("packet is not explicit player movement intent");
    }
    if mover == EntityId(0) || expected_mover != Some(mover) {
        return Err("movement mover GUID does not match the active player");
    }
    if flags & 0x3 == 0x3
        || flags & 0xC == 0xC
        || flags & 0x30 == 0x30
        || flags & 0xC0 == 0xC0
        || flags & 0x00C0_0000 == 0x00C0_0000
    {
        return Err("movement flags contain contradictory directions");
    }
    const KNOWN_MOVEMENT_FLAGS: u32 = 0x7FFF_FFFF;
    const ROOT: u32 = 0x0000_0800;
    const MOVING_FLAGS: u32 = 0x04C0_30CF;
    if flags & !KNOWN_MOVEMENT_FLAGS != 0 {
        return Err("movement flags contain an unsupported bit");
    }
    if flags & ROOT != 0 && flags & MOVING_FLAGS != 0 {
        return Err("rooted movement cannot contain active movement flags");
    }
    let required = match opcode {
        0x0B5 => Some(0x01),        // start forward
        0x0B6 => Some(0x02),        // start backward
        0x0B8 => Some(0x04),        // start strafe left
        0x0B9 => Some(0x08),        // start strafe right
        0x0BB => Some(0x0000_1000), // jump/falling movement info
        0x0BC => Some(0x10),        // start turn left
        0x0BD => Some(0x20),        // start turn right
        0x0BF => Some(0x40),        // start pitch up
        0x0C0 => Some(0x80),        // start pitch down
        0x0CA => Some(0x0020_0000), // start swim
        0x359 => Some(0x0040_0000), // start ascend
        0x3A7 => Some(0x0080_0000), // start descend
        0x0DA | 0x0DB => None,      // set facing / pitch: movement state may be unchanged
        _ => return Err("opcode has no takeover flag policy"),
    };
    if required.is_some_and(|flag| flags & flag == 0) {
        return Err("movement opcode does not match movement flags");
    }
    Ok(())
}

#[cfg(test)]
mod movement_clock_tests {
    use super::*;

    #[test]
    fn typed_travel_cleanup_uses_wotlk_mount_and_aura_cancel_commands() {
        let mut clock = MovementClock::default();
        let (mount, _) = encode_gameplay_command(
            GameplayCommand::CancelMount,
            Some(EntityId(7)),
            None,
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(mount.opcode, 0x0375);
        assert!(mount.body.is_empty());

        let (aura, _) = encode_gameplay_command(
            GameplayCommand::CancelAura { spell: 783 },
            Some(EntityId(7)),
            None,
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(aura.opcode, 0x0136);
        assert_eq!(aura.body, 783_u32.to_le_bytes());
    }

    #[test]
    fn repair_all_uses_the_observed_vendor_and_never_uses_guild_funds() {
        let mut clock = MovementClock::default();
        let vendor = EntityId(0x1122334455667788);
        let (packet, _) = encode_gameplay_command(
            GameplayCommand::RepairEquipment { vendor },
            Some(EntityId(7)),
            None,
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();

        assert_eq!(packet.opcode, 0x02A8);
        assert_eq!(packet.body.len(), 17);
        assert_eq!(&packet.body[..8], &vendor.0.to_le_bytes());
        assert_eq!(&packet.body[8..16], &0_u64.to_le_bytes());
        assert_eq!(packet.body[16], 0);
    }

    #[test]
    fn bank_actions_use_wrath_banker_and_backpack_slot_layouts() {
        let mut clock = MovementClock::default();
        let banker = EntityId(0x1122_3344_5566_7788);
        let (open, _) = encode_gameplay_command(
            GameplayCommand::BankActivate { banker },
            Some(EntityId(7)),
            None,
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(open.opcode, 0x01B7);
        assert_eq!(open.body, banker.0.to_le_bytes());

        let (deposit, _) = encode_gameplay_command(
            GameplayCommand::BankDeposit {
                banker,
                item: 2589,
                item_guid: EntityId(99),
                backpack_slot: 23,
            },
            Some(EntityId(7)),
            None,
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(deposit.opcode, 0x0283);
        assert_eq!(deposit.body, [0xff, 23]);
    }

    #[test]
    fn mailbox_actions_match_wrath_packet_layouts() {
        let mailbox = EntityId(0x1122_3344_5566_7788);
        let mut clock = MovementClock::default();
        let (list, _) = encode_gameplay_command(
            GameplayCommand::MailboxList { mailbox },
            Some(EntityId(7)),
            None,
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(list.opcode, 0x023A);
        assert_eq!(list.body, mailbox.0.to_le_bytes());

        let (money, _) = encode_gameplay_command(
            GameplayCommand::MailTake {
                mailbox_generation: 4,
                mailbox,
                mail_id: 77,
                target: wow_domain::MailTakeTarget::Money,
            },
            Some(EntityId(7)),
            None,
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(money.opcode, 0x0245);
        assert_eq!(&money.body[..8], &mailbox.0.to_le_bytes());
        assert_eq!(&money.body[8..], &77_u32.to_le_bytes());

        let (item, _) = encode_gameplay_command(
            GameplayCommand::MailTake {
                mailbox_generation: 4,
                mailbox,
                mail_id: 77,
                target: wow_domain::MailTakeTarget::Attachment { low_guid: 9001 },
            },
            Some(EntityId(7)),
            None,
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(item.opcode, 0x0246);
        assert_eq!(&item.body[..8], &mailbox.0.to_le_bytes());
        assert_eq!(&item.body[8..12], &77_u32.to_le_bytes());
        assert_eq!(&item.body[12..], &9001_u32.to_le_bytes());
    }

    #[test]
    fn vendor_purchase_uses_observed_slot_item_and_one_lot_count() {
        let mut clock = MovementClock::default();
        let vendor = EntityId(0x1122334455667788);
        let (packet, _) = encode_gameplay_command(
            GameplayCommand::VendorBuy {
                vendor,
                item: 6947,
                slot: 3,
                count: 1,
            },
            Some(EntityId(7)),
            None,
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();

        assert_eq!(packet.opcode, 0x01a2);
        assert_eq!(&packet.body[..8], &vendor.0.to_le_bytes());
        assert_eq!(&packet.body[8..12], &6947_u32.to_le_bytes());
        assert_eq!(&packet.body[12..16], &3_u32.to_le_bytes());
        assert_eq!(&packet.body[16..20], &1_u32.to_le_bytes());
        assert_eq!(packet.body[20], 0);
    }

    #[test]
    fn set_ammo_uses_projectile_entry_payload() {
        let mut clock = MovementClock::default();
        let (packet, _) = encode_gameplay_command(
            GameplayCommand::SetAmmo { item: 2512 },
            Some(EntityId(7)),
            None,
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();

        assert_eq!(packet.opcode, 0x0268);
        assert_eq!(packet.body, 2512_u32.to_le_bytes());
    }

    #[test]
    fn trainer_list_and_buy_use_wrath_guid_and_spell_layouts() {
        let mut clock = MovementClock::default();
        let trainer = EntityId(0x1122334455667788);
        let (list, _) = encode_gameplay_command(
            GameplayCommand::TrainerList { trainer },
            Some(EntityId(7)),
            None,
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(list.opcode, 0x01b0);
        assert_eq!(list.body, trainer.0.to_le_bytes());

        let (buy, _) = encode_gameplay_command(
            GameplayCommand::TrainerBuy {
                trainer,
                spell: 1234,
            },
            Some(EntityId(7)),
            None,
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(buy.opcode, 0x01b2);
        assert_eq!(&buy.body[..8], &trainer.0.to_le_bytes());
        assert_eq!(&buy.body[8..], &1234_u32.to_le_bytes());
    }

    #[test]
    fn item_targeted_imbues_and_consumables_use_wrath_item_target_masks() {
        let mut clock = MovementClock::default();
        let weapon = EntityId(0x1122334455667788);
        let (cast, _) = encode_gameplay_command(
            GameplayCommand::CastOnItem {
                spell: 8232,
                item_guid: weapon,
            },
            Some(EntityId(7)),
            None,
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(cast.opcode, 0x012f);
        assert_eq!(&cast.body[..6], &[0, 0x28, 0x20, 0, 0, 0]);
        assert_eq!(&cast.body[6..10], &0x10_u32.to_le_bytes());
        assert_eq!(cast.body.len(), 19);

        let poison = EntityId(0x0102030405060708);
        let (use_item, _) = encode_gameplay_command(
            GameplayCommand::UseItemOnItem {
                item: poison.0 as u32,
                item_guid: poison,
                backpack_slot: 23,
                spell: 11343,
                target_item_guid: weapon,
            },
            Some(EntityId(7)),
            None,
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(use_item.opcode, 0x00ab);
        assert_eq!(&use_item.body[..3], &[0xff, 23, 0]);
        assert_eq!(&use_item.body[3..7], &11343_u32.to_le_bytes());
        assert_eq!(&use_item.body[20..24], &0x10_u32.to_le_bytes());
        assert_eq!(use_item.body.len(), 33);
    }

    #[test]
    fn equipment_and_pet_maintenance_use_wrath_wire_layouts() {
        let mut clock = MovementClock::default();
        let (equip, _) = encode_gameplay_command(
            GameplayCommand::EquipItem {
                item_guid: EntityId(0x1122334455667788),
                destination_slot: 10,
            },
            Some(EntityId(7)),
            None,
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(equip.opcode, 0x010f);
        assert_eq!(
            equip.body,
            [0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 10]
        );

        let (stance, _) = encode_gameplay_command(
            GameplayCommand::PetSetReaction {
                pet: EntityId(9),
                reaction: 1,
            },
            Some(EntityId(7)),
            None,
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(stance.opcode, 0x0175);
        assert_eq!(&stance.body[8..12], &0x0600_0001_u32.to_le_bytes());

        let (attack, _) = encode_gameplay_command(
            GameplayCommand::PetAttack {
                pet: EntityId(9),
                target: EntityId(10),
            },
            Some(EntityId(7)),
            None,
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(attack.opcode, 0x0175);
        assert_eq!(&attack.body[8..12], &0x0700_0002_u32.to_le_bytes());
        assert_eq!(&attack.body[12..20], &10_u64.to_le_bytes());

        let (autocast, _) = encode_gameplay_command(
            GameplayCommand::PetSetAutocast {
                pet: EntityId(9),
                spell: 133,
                enabled: true,
            },
            Some(EntityId(7)),
            None,
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(autocast.opcode, 0x02f3);
        assert_eq!(autocast.body, [9, 0, 0, 0, 0, 0, 0, 0, 133, 0, 0, 0, 1]);
    }

    #[test]
    fn warlock_pet_summon_uses_implicit_self_target_encoding() {
        let mut clock = MovementClock::default();
        let (frame, movement) = encode_gameplay_command(
            GameplayCommand::SummonPet {
                spell: 688,
                player: EntityId(7),
            },
            Some(EntityId(7)),
            None,
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();

        assert_eq!(frame.opcode, 0x012E);
        assert_eq!(frame.body, encode_cast_spell(688, None, 0x0000_0002));
        assert_eq!(frame.body.len(), 10);
        assert!(movement.is_none());
    }

    #[test]
    fn quest_item_gameobject_open_matches_the_captured_client_cast() {
        let target = EntityId(0xF110_0244_1300_0C92);
        let mut clock = MovementClock::default();
        let (frame, movement) = encode_gameplay_command(
            GameplayCommand::CastGameObject {
                spell: wow_domain::QUEST_ITEM_GAMEOBJECT_OPEN_SPELL_ID,
                target,
                report_use: true,
            },
            None,
            None,
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();

        assert_eq!(frame.opcode, 0x012E);
        assert_eq!(
            frame.body,
            [
                0x00, // cast count
                0x4E, 0x19, 0x00, 0x00, // Spell 6478 from the captured client packet
                0x00, // cast flags
                0x00, 0x08, 0x00, 0x00, // game-object target mask
                0xFB, 0x92, 0x0C, 0x13, 0x44, 0x02, 0x10, 0xF1, // packed GUID
            ]
        );
        assert!(movement.is_none());
    }

    #[test]
    fn facing_command_normalizes_negative_orientation() {
        let point = Vec3::new(1.0, 2.0, 3.0);
        let mut clock = MovementClock::default();
        let (frame, update) = encode_gameplay_command(
            GameplayCommand::FaceDirection { orientation: -1.0 },
            Some(EntityId(7)),
            Some(WorldPosition {
                map: 0,
                point,
                orientation: 0.0,
            }),
            0,
            &mut clock,
        )
        .unwrap()
        .unwrap();

        let (_, _, _, _, encoded_orientation) =
            decode_simple_movement(u32::from(frame.opcode), &frame.body).unwrap();
        let expected = Radians::normalized(-1.0).unwrap().0;
        assert_eq!(frame.opcode, 0x00DA);
        assert!((encoded_orientation - expected).abs() < f32::EPSILON);
        assert_eq!(update.unwrap().0.orientation, expected);
    }

    #[test]
    fn generated_timestamps_use_observed_client_clock_and_stay_monotonic() {
        let mut clock = MovementClock::default();
        clock.observe(42_000);

        let first = clock.next_timestamp();
        let second = clock.next_timestamp();

        assert!(timestamp_is_after_or_equal(first, 42_000));
        assert!(timestamp_is_after(second, first));
    }

    #[test]
    fn generated_timestamps_remain_monotonic_across_wrap() {
        let mut clock = MovementClock {
            client_time_ms: u32::MAX - 2,
            anchored_at: Instant::now() - Duration::from_millis(5),
            last_generated: Some(u32::MAX - 2),
        };

        let timestamp = clock.next_timestamp();

        assert!(timestamp_is_after(timestamp, u32::MAX - 2));
    }

    #[test]
    fn stale_client_sample_does_not_move_the_clock_backwards() {
        let mut clock = MovementClock::default();
        clock.observe(42_000);
        let first = clock.next_timestamp();

        clock.observe(10);
        let second = clock.next_timestamp();

        assert!(timestamp_is_after(second, first));
    }

    #[test]
    fn time_sync_response_round_trips_counter_and_client_timestamp() {
        let frame = encode_time_sync_response(7, 12_345);

        assert_eq!(frame.opcode, CMSG_TIME_SYNC_RESP_OPCODE);
        assert_eq!(parse_time_sync_response(&frame.body), Some((7, 12_345)));
        assert_eq!(parse_time_sync_response(&frame.body[..7]), None);
    }

    #[test]
    fn time_sync_request_reads_counter_and_rejects_short_body() {
        assert_eq!(parse_time_sync_request(&7_u32.to_le_bytes()), Some(7));
        assert_eq!(parse_time_sync_request(&[7, 0, 0]), None);
    }

    #[test]
    fn movement_decoder_requires_complete_conditional_fields_and_no_trailing_bytes() {
        let body = encode_simple_movement(EntityId(7), 0x01, 12, Vec3::new(1.0, 2.0, 3.0), 0.5);
        assert!(decode_simple_movement(0x0B5, &body).is_some());

        let mut truncated = body.clone();
        truncated.pop();
        assert!(decode_simple_movement(0x0B5, &truncated).is_none());

        let mut trailing = body.clone();
        trailing.push(0);
        assert!(decode_simple_movement(0x0B5, &trailing).is_none());

        let falling =
            encode_simple_movement(EntityId(7), 0x1000, 12, Vec3::new(1.0, 2.0, 3.0), 0.5);
        assert!(decode_simple_movement(0x0BB, &falling).is_none());
        let mut complete_jump = falling;
        for value in [1.0_f32, 0.25, 0.75, 7.0] {
            complete_jump.extend_from_slice(&value.to_le_bytes());
        }
        assert!(decode_simple_movement(0x0BB, &complete_jump).is_some());
    }

    #[test]
    fn takeover_flag_rules_cover_each_explicit_start_opcode() {
        let cases = [
            (0x0B5, 0x01),
            (0x0B6, 0x02),
            (0x0B8, 0x04),
            (0x0B9, 0x08),
            (0x0BB, 0x1000),
            (0x0BC, 0x10),
            (0x0BD, 0x20),
            (0x0BF, 0x40),
            (0x0C0, 0x80),
            (0x0CA, 0x0020_0000),
            (0x359, 0x0040_0000),
            (0x3A7, 0x0080_0000),
        ];
        for (opcode, required) in cases {
            assert!(
                validate_player_takeover(opcode, EntityId(7), required, Some(EntityId(7))).is_ok(),
                "opcode {opcode:#x}"
            );
            assert!(
                validate_player_takeover(opcode, EntityId(7), 0, Some(EntityId(7))).is_err(),
                "opcode {opcode:#x}"
            );
        }
        for contradictory in [0x03, 0x0C, 0x30, 0xC0, 0x00C0_0000] {
            assert!(
                validate_player_takeover(0x0DA, EntityId(7), contradictory, Some(EntityId(7)))
                    .is_err()
            );
        }
        assert!(
            validate_player_takeover(0x0B5, EntityId(7), 0x8000_0001, Some(EntityId(7))).is_err()
        );
        assert!(
            validate_player_takeover(0x0B5, EntityId(7), 0x0000_0801, Some(EntityId(7))).is_err()
        );
        assert!(validate_player_takeover(0x0B7, EntityId(7), 0, Some(EntityId(7))).is_err());
    }

    #[test]
    fn recent_bot_echo_match_takes_precedence_over_takeover_classification() {
        assert!(should_suppress_bot_echo(true));
        assert!(!should_suppress_bot_echo(false));
    }

    #[test]
    fn near_teleport_ack_requires_exact_packed_guid_flags_time_layout() {
        let mut body = Vec::new();
        push_packed_guid(&mut body, EntityId(7));
        body.extend_from_slice(&0x20_u32.to_le_bytes());
        body.extend_from_slice(&99_u32.to_le_bytes());
        assert_eq!(
            decode_near_teleport_ack(&body),
            Some((EntityId(7), 0x20, 99))
        );
        body.push(0);
        assert_eq!(decode_near_teleport_ack(&body), None);
    }

    #[test]
    fn movement_correction_ack_requires_matching_request_and_exact_payload() {
        let mut request = Vec::new();
        push_packed_guid(&mut request, EntityId(7));
        request.extend_from_slice(&19_u32.to_le_bytes());
        let expected = movement_correction_expectation(0x00E2, &request).unwrap();
        assert_eq!(expected.ack_opcode, 0x00E3);

        let movement = encode_simple_movement(EntityId(7), 0, 12, Vec3::new(1.0, 2.0, 3.0), 0.5);
        let mut ack = Vec::new();
        push_packed_guid(&mut ack, EntityId(7));
        ack.extend_from_slice(&19_u32.to_le_bytes());
        ack.extend_from_slice(&movement[2..]);
        ack.extend_from_slice(&7.0_f32.to_le_bytes());
        assert_eq!(
            validate_movement_correction_ack(0x00E3, &ack, expected),
            Ok(EntityId(7))
        );

        let mut wrong_counter = ack.clone();
        wrong_counter[2] = 20;
        assert!(validate_movement_correction_ack(0x00E3, &wrong_counter, expected).is_err());

        let mut malformed_tail = ack;
        malformed_tail.pop();
        assert!(validate_movement_correction_ack(0x00E3, &malformed_tail, expected).is_err());
    }

    #[test]
    fn bot_destination_limits_follow_elapsed_time_and_world_bounds() {
        let origin = WorldPosition {
            map: 0,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        };
        assert!(
            validate_movement_position(
                Some(origin),
                Vec3::new(3.0, 0.0, 0.0),
                100,
                MovementSource::Bot
            )
            .is_ok()
        );
        assert!(
            validate_movement_position(
                Some(origin),
                Vec3::new(4.0, 0.0, 0.0),
                100,
                MovementSource::Bot
            )
            .is_err()
        );
        assert!(
            validate_movement_position(
                None,
                Vec3::new(200_001.0, 0.0, 0.0),
                0,
                MovementSource::Bot
            )
            .is_err()
        );
    }

    #[test]
    fn server_motion_projection_emits_monster_move_and_terminal_stop() {
        let origin = WorldPosition {
            map: 0,
            point: Vec3::new(1.0, 2.0, 3.0),
            orientation: 0.0,
        };
        let destination = WorldPosition {
            map: 0,
            point: Vec3::new(2.0, 2.0, 3.0),
            orientation: 0.2,
        };
        let mut projection = ServerMotionProjection::default();
        projection.reset_to(EntityId(7), origin, 10);
        assert!(
            projection
                .project(EntityId(7), destination, 30, true)
                .is_none()
        );
        let movement = projection
            .project(EntityId(7), destination, 110, true)
            .unwrap();
        assert_eq!(movement.opcode, 0x00DD);
        assert!(movement.body.len() > 30);
        let stop = projection.stop(EntityId(7), destination).unwrap();
        assert_eq!(stop.opcode, 0x00DD);
        assert_eq!(stop.body.last(), Some(&1));
    }
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
