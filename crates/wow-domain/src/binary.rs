/// Read a fixed-size byte array at `offset` without advancing caller state.
///
/// Packet readers can use this helper for bounded extraction, then apply
/// their own error type and cursor update rules.
pub fn array_at<const N: usize>(bytes: &[u8], offset: usize) -> Option<[u8; N]> {
    let end = offset.checked_add(N)?;
    bytes.get(offset..end)?.try_into().ok()
}

/// Read an unsigned integer from a fixed-size little-endian byte array.
pub fn u16_le(bytes: &[u8]) -> Option<u16> {
    Some(u16::from_le_bytes(array_at(bytes, 0)?))
}

/// Read an unsigned integer from a fixed-size little-endian byte array.
pub fn u32_le(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(array_at(bytes, 0)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_little_endian_reads_reject_short_slices() {
        assert_eq!(u16_le(&[0x34, 0x12]), Some(0x1234));
        assert_eq!(u32_le(&[0x78, 0x56, 0x34, 0x12]), Some(0x12345678));
        assert_eq!(u16_le(&[0x34]), None);
        assert_eq!(u32_le(&[0x78, 0x56, 0x34]), None);
        assert_eq!(array_at::<2>(&[0, 1, 2], usize::MAX), None);
    }
}
