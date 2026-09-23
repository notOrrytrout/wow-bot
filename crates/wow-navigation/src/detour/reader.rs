use std::fmt::{Display, Formatter};

const MAX_STRING_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PacketReadError {
    Truncated { offset: usize, needed: usize },
    InvalidUtf8 { offset: usize },
    UnterminatedString { offset: usize },
    StringTooLong { offset: usize, limit: usize },
    CountTooLarge { value: usize, limit: usize },
    InvalidValue(&'static str),
}

impl Display for PacketReadError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated { offset, needed } => {
                write!(
                    f,
                    "packet is truncated at byte {offset}; needs {needed} bytes"
                )
            }
            Self::InvalidUtf8 { offset } => {
                write!(f, "packet string at byte {offset} is not UTF-8")
            }
            Self::UnterminatedString { offset } => {
                write!(f, "packet string at byte {offset} has no terminator")
            }
            Self::StringTooLong { offset, limit } => {
                write!(f, "packet string at byte {offset} exceeds {limit} bytes")
            }
            Self::CountTooLarge { value, limit } => {
                write!(f, "packet count {value} exceeds limit {limit}")
            }
            Self::InvalidValue(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for PacketReadError {}

pub type PacketReadResult<T> = Result<T, PacketReadError>;

#[derive(Debug, Clone)]
pub struct PacketReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> PacketReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    pub fn at(bytes: &'a [u8], offset: usize) -> PacketReadResult<Self> {
        if offset > bytes.len() {
            return Err(PacketReadError::Truncated { offset, needed: 0 });
        }
        Ok(Self { bytes, offset })
    }

    pub fn take(&mut self, length: usize) -> PacketReadResult<&'a [u8]> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(PacketReadError::InvalidValue("packet offset overflow"))?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(PacketReadError::Truncated {
                offset: self.offset,
                needed: length,
            })?;
        self.offset = end;
        Ok(value)
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    pub fn skip(&mut self, length: usize) -> PacketReadResult<()> {
        self.take(length).map(|_| ())
    }

    pub fn u8(&mut self) -> PacketReadResult<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> PacketReadResult<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub fn u32(&mut self) -> PacketReadResult<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn i32(&mut self) -> PacketReadResult<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn u64(&mut self) -> PacketReadResult<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn packed_guid(&mut self) -> PacketReadResult<u64> {
        let mask = self.u8()?;
        let mut guid = 0u64;
        for index in 0..8 {
            if mask & (1 << index) != 0 {
                guid |= (self.u8()? as u64) << (index * 8);
            }
        }
        Ok(guid)
    }

    pub fn f32(&mut self) -> PacketReadResult<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn bounded_count(&mut self, value: usize, limit: usize) -> PacketReadResult<usize> {
        if value > limit {
            return Err(PacketReadError::CountTooLarge { value, limit });
        }
        Ok(value)
    }

    pub fn cstring(&mut self) -> PacketReadResult<&'a str> {
        let start = self.offset;
        let remaining = self.bytes.get(start..).unwrap_or_default();
        let length = remaining
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(PacketReadError::UnterminatedString { offset: start })?;
        if length > MAX_STRING_BYTES {
            return Err(PacketReadError::StringTooLong {
                offset: start,
                limit: MAX_STRING_BYTES,
            });
        }
        let value = std::str::from_utf8(&remaining[..length])
            .map_err(|_| PacketReadError::InvalidUtf8 { offset: start })?;
        self.offset = start + length + 1;
        Ok(value)
    }
}
