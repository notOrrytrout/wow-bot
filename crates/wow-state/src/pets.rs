use crate::entities::Entities;
use serde::{Deserialize, Serialize};
use wow_domain::EntityId;

/// Decoded controlled-unit spell bar from an `SMSG_PET_SPELLS` packet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ControlledUnitSpellBar {
    pub mover: Option<EntityId>,
    pub reaction: Option<u8>,
    pub abilities: Vec<PetAbilityState>,
}

/// Decode the WotLK controlled-unit spell bar shared by the proxy and adapter.
/// A zero GUID confirms that no controllable pet is active; a nonzero GUID
/// requires the complete fixed spell bar before it can be used as evidence.
pub fn parse_controlled_unit_spell_bar(body: &[u8]) -> Option<ControlledUnitSpellBar> {
    let guid = wow_domain::binary::u64_le_at(body, 0)?;
    if guid == 0 {
        return Some(ControlledUnitSpellBar::default());
    }
    if body.len() < 58 {
        return None;
    }

    let mut abilities = Vec::new();
    for index in 0..10 {
        let packed = wow_domain::binary::u32_le_at(body, 18 + index * 4)?;
        let spell = packed & 0x00FF_FFFF;
        let autocast = match (packed >> 24) as u8 {
            0xC1 => Some(true),
            0x81 => Some(false),
            _ => None,
        };
        if spell != 0
            && !abilities
                .iter()
                .any(|ability: &PetAbilityState| ability.spell == spell)
        {
            abilities.push(PetAbilityState { spell, autocast });
        }
    }

    Some(ControlledUnitSpellBar {
        mover: Some(EntityId(guid)),
        reaction: Some(body[14]),
        abilities,
    })
}

/// Server-reported pet ability and autocast state from SMSG_PET_SPELLS.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PetAbilityState {
    pub spell: u32,
    /// `None` means the ability is passive or the server did not report its autocast state.
    #[serde(default)]
    pub autocast: Option<bool>,
}

/// Server-confirmed state for the player's current controllable pet.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PetState {
    /// False until the server confirms pet presence or absence.
    #[serde(default)]
    pub control_known: bool,
    #[serde(default)]
    pub guid: Option<EntityId>,
    /// WotLK reaction state: 0 passive, 1 defensive, 2 aggressive.
    #[serde(default)]
    pub reaction: Option<u8>,
    /// Pet spell-bar state. It is authoritative after SMSG_PET_SPELLS.
    #[serde(default)]
    pub abilities: Vec<PetAbilityState>,
}

impl PetState {
    /// `None` means the server has not confirmed pet presence or absence.
    pub fn has_active_pet(&self, entities: &Entities) -> Option<bool> {
        if !self.control_known {
            return None;
        }
        let Some(guid) = self.guid else {
            return Some(false);
        };
        Some(!entities.0.get(&guid).is_some_and(|entity| entity.is_dead()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pet_spell_packet_requires_complete_bar_and_keeps_unique_abilities() {
        let mut body = vec![0; 58];
        body[..8].copy_from_slice(&55_u64.to_le_bytes());
        body[14] = 2;
        body[18..22].copy_from_slice(&0xC100_0085_u32.to_le_bytes());
        body[22..26].copy_from_slice(&0x8100_0085_u32.to_le_bytes());
        body[26..30].copy_from_slice(&0x8100_0123_u32.to_le_bytes());

        let packet = parse_controlled_unit_spell_bar(&body).expect("complete pet spell bar");
        assert_eq!(packet.mover, Some(EntityId(55)));
        assert_eq!(packet.reaction, Some(2));
        assert_eq!(
            packet.abilities,
            vec![
                PetAbilityState {
                    spell: 133,
                    autocast: Some(true)
                },
                PetAbilityState {
                    spell: 291,
                    autocast: Some(false)
                },
            ]
        );
        assert!(parse_controlled_unit_spell_bar(&body[..57]).is_none());
        assert_eq!(
            parse_controlled_unit_spell_bar(&[0; 8]),
            Some(ControlledUnitSpellBar::default())
        );
        assert!(parse_controlled_unit_spell_bar(&body[..7]).is_none());
    }
}
