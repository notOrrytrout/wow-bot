use wow_domain::binary::{u16_le_at, u32_le_at};

pub(super) fn read_packed_guid(body: &[u8], cursor: &mut usize) -> Option<u64> {
    let mask = *body.get(*cursor)?;
    *cursor = cursor.checked_add(1)?;
    let mut guid = [0_u8; 8];
    for (index, byte) in guid.iter_mut().enumerate() {
        if mask & (1 << index) != 0 {
            *byte = *body.get(*cursor)?;
            *cursor = cursor.checked_add(1)?;
        }
    }
    Some(u64::from_le_bytes(guid))
}

pub(super) fn read_u16_cursor(bytes: &[u8], cursor: &mut usize) -> Option<u16> {
    let value = u16_le_at(bytes, *cursor)?;
    *cursor = cursor.checked_add(2)?;
    Some(value)
}

pub(super) fn read_u32_cursor(bytes: &[u8], cursor: &mut usize) -> Option<u32> {
    let value = u32_le_at(bytes, *cursor)?;
    *cursor = cursor.checked_add(4)?;
    Some(value)
}

pub(super) fn read_f32_cursor(bytes: &[u8], cursor: &mut usize) -> Option<f32> {
    let value = f32::from_bits(read_u32_cursor(bytes, cursor)?);
    value.is_finite().then_some(value)
}
