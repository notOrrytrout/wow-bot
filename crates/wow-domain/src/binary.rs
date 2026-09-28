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

pub fn u16_le_at(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(array_at(bytes, offset)?))
}

pub fn u32_le_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(array_at(bytes, offset)?))
}

pub fn u64_le_at(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(array_at(bytes, offset)?))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CStringReadError {
    Unterminated,
    TooLong { length: usize, limit: usize },
    InvalidUtf8,
}

/// Read a null-terminated UTF-8 string and return its first byte after the terminator.
pub fn cstring_at(
    bytes: &[u8],
    offset: usize,
    max_length: Option<usize>,
) -> Result<(&str, usize), CStringReadError> {
    let rest = bytes.get(offset..).ok_or(CStringReadError::Unterminated)?;
    let length = rest
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(CStringReadError::Unterminated)?;
    if let Some(limit) = max_length.filter(|limit| length > *limit) {
        return Err(CStringReadError::TooLong { length, limit });
    }
    let value = std::str::from_utf8(&rest[..length]).map_err(|_| CStringReadError::InvalidUtf8)?;
    let next = offset
        .checked_add(length)
        .and_then(|end| end.checked_add(1))
        .ok_or(CStringReadError::Unterminated)?;
    Ok((value, next))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_little_endian_reads_reject_short_slices() {
        assert_eq!(u16_le(&[0x34, 0x12]), Some(0x1234));
        assert_eq!(u32_le(&[0x78, 0x56, 0x34, 0x12]), Some(0x12345678));
        assert_eq!(u32_le_at(&[0, 0x78, 0x56, 0x34, 0x12], 1), Some(0x12345678));
        assert_eq!(
            u64_le_at(&[1, 2, 3, 4, 5, 6, 7, 8], 0),
            Some(0x0807060504030201)
        );
        assert_eq!(u16_le(&[0x34]), None);
        assert_eq!(u32_le(&[0x78, 0x56, 0x34]), None);
        assert_eq!(array_at::<2>(&[0, 1, 2], usize::MAX), None);
    }

    #[test]
    fn cstring_reader_reports_validation_errors_and_next_offset() {
        assert_eq!(cstring_at(b"x\0tail", 0, Some(1)), Ok(("x", 2)));
        assert_eq!(
            cstring_at(b"xx\0", 0, Some(1)),
            Err(CStringReadError::TooLong {
                length: 2,
                limit: 1
            })
        );
        assert_eq!(
            cstring_at(b"\xff\0", 0, None),
            Err(CStringReadError::InvalidUtf8)
        );
        assert_eq!(
            cstring_at(b"unterminated", 0, None),
            Err(CStringReadError::Unterminated)
        );
        assert_eq!(
            cstring_at(b"x", usize::MAX, None),
            Err(CStringReadError::Unterminated)
        );
    }
}
