//! Headless Warden client for AzerothCore's fixed WotLK Windows module.
//! Unknown modules and checks fail closed. No Warden packet bodies are logged.

use super::relay::{Rc4, keys};
use crate::crypt::session::SessionKey;
use anyhow::{Context, Result, bail, ensure};
use md5::{Digest as _, Md5};
use sha1::Sha1;
use std::{fs, path::Path};

const MODULE_ID: [u8; 16] = [
    0x79, 0xC0, 0x76, 0x8D, 0x65, 0x79, 0x77, 0xD6, 0x97, 0xE1, 0x0B, 0xAD, 0x95, 0x6C, 0xCE, 0xD1,
];
const MODULE_SEED: [u8; 16] = [
    0x4D, 0x80, 0x8D, 0x2C, 0x77, 0xD9, 0x05, 0xC4, 0x1A, 0x63, 0x80, 0xEC, 0x08, 0x58, 0x6A, 0xFE,
];
const CLIENT_HASH: [u8; 20] = [
    0x56, 0x8C, 0x05, 0x4C, 0x78, 0x1A, 0x97, 0x2A, 0x60, 0x37, 0xA2, 0x29, 0x0C, 0x22, 0xB5, 0x25,
    0x71, 0xA0, 0x6F, 0x4E,
];
const CLIENT_KEY: [u8; 16] = [
    0x7F, 0x96, 0xEE, 0xFD, 0xA5, 0xB6, 0x3D, 0x20, 0xA4, 0xDF, 0x8E, 0x00, 0xCB, 0xF4, 0x83, 0x04,
];
const SERVER_KEY: [u8; 16] = [
    0xC2, 0xB7, 0xAD, 0xED, 0xFC, 0xCC, 0xA9, 0xC2, 0xBF, 0xB3, 0xF8, 0x56, 0x02, 0xBA, 0x80, 0x9B,
];
const MAX_MODULE_SIZE: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    AwaitModule,
    DownloadModule,
    AwaitHash,
    AwaitChecks,
}

pub struct WardenReply {
    pub event: &'static str,
    pub body: Option<Vec<u8>>,
}

pub struct WardenClient {
    input: Rc4,
    output: Rc4,
    phase: Phase,
    module: Vec<u8>,
    module_size: usize,
    image: Option<PeImage>,
}

impl WardenClient {
    pub fn new(session_key: &SessionKey, client_image: Option<&Path>) -> Result<Self> {
        let (input, output) = keys(session_key);
        let image = client_image.map(PeImage::load).transpose()?;
        Ok(Self {
            input: Rc4::new(&output),
            output: Rc4::new(&input),
            phase: Phase::AwaitModule,
            module: Vec::new(),
            module_size: 0,
            image,
        })
    }

    pub fn handle(&mut self, encrypted: &[u8]) -> Result<WardenReply> {
        let mut body = encrypted.to_vec();
        self.input.apply(&mut body);
        let (&command, payload) = body.split_first().context("empty Warden packet")?;
        match command {
            0 => {
                ensure!(
                    self.phase == Phase::AwaitModule,
                    "unexpected Warden module request"
                );
                ensure!(payload.len() == 36, "invalid Warden module request length");
                ensure!(payload[..16] == MODULE_ID, "unsupported Warden module ID");
                let size = u32::from_le_bytes(payload[32..36].try_into()?) as usize;
                ensure!(
                    size > 0 && size <= MAX_MODULE_SIZE,
                    "invalid Warden module size"
                );
                self.module_size = size;
                self.module.clear();
                self.phase = Phase::DownloadModule;
                Ok(self.reply("module requested", Some(&[0])))
            }
            1 => {
                ensure!(
                    self.phase == Phase::DownloadModule,
                    "unexpected Warden module data"
                );
                ensure!(payload.len() >= 2, "short Warden module data");
                let chunk_size = u16::from_le_bytes(payload[..2].try_into()?) as usize;
                ensure!(
                    chunk_size == payload.len() - 2,
                    "invalid Warden module chunk length"
                );
                ensure!(
                    self.module.len() + chunk_size <= self.module_size,
                    "Warden module exceeds advertised size"
                );
                self.module.extend_from_slice(&payload[2..]);
                if self.module.len() != self.module_size {
                    return Ok(WardenReply {
                        event: "module chunk",
                        body: None,
                    });
                }
                let digest: [u8; 16] = Md5::digest(&self.module).into();
                ensure!(digest == MODULE_ID, "Warden module checksum mismatch");
                self.phase = Phase::AwaitHash;
                Ok(self.reply("module verified", Some(&[1])))
            }
            5 => {
                ensure!(
                    self.phase == Phase::AwaitHash,
                    "unexpected Warden hash request"
                );
                ensure!(payload == MODULE_SEED, "unsupported Warden hash seed");
                let mut response = Vec::with_capacity(21);
                response.push(4);
                response.extend_from_slice(&CLIENT_HASH);
                let result = self.reply("hash verified", Some(&response));
                self.input = Rc4::new(&SERVER_KEY);
                self.output = Rc4::new(&CLIENT_KEY);
                self.phase = Phase::AwaitChecks;
                Ok(result)
            }
            3 => {
                ensure!(
                    self.phase == Phase::AwaitChecks,
                    "Warden module initialized before hash"
                );
                Ok(WardenReply {
                    event: "module initialized",
                    body: None,
                })
            }
            2 => {
                ensure!(
                    self.phase == Phase::AwaitChecks,
                    "Warden checks before hash"
                );
                let image = self
                    .image
                    .as_ref()
                    .context("Warden checks require proxy.warden_client_image")?;
                let response = checks_response(payload, image)?;
                Ok(self.reply("checks answered", Some(&response)))
            }
            _ => bail!("unsupported Warden server command {command}"),
        }
    }

    fn reply(&mut self, event: &'static str, plain: Option<&[u8]>) -> WardenReply {
        let body = plain.map(|plain| {
            let mut encrypted = plain.to_vec();
            self.output.apply(&mut encrypted);
            encrypted
        });
        WardenReply { event, body }
    }
}

fn checks_response(request: &[u8], image: &PeImage) -> Result<Vec<u8>> {
    ensure!(request.len() >= 3, "short Warden checks request");
    let mut cursor = 0;
    let mut strings = Vec::new();
    loop {
        let len = *request
            .get(cursor)
            .context("truncated Warden string table")? as usize;
        cursor += 1;
        if len == 0 {
            break;
        }
        let bytes = request
            .get(cursor..cursor + len)
            .context("truncated Warden string")?;
        strings.push(bytes);
        cursor += len;
    }
    let xor = *request.last().unwrap();
    let mut results = Vec::new();
    let end = request.len() - 1;
    while cursor < end {
        let kind = request[cursor] ^ xor;
        cursor += 1;
        match kind {
            0x57 => {
                results.push(1);
                let tick = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u32;
                results.extend_from_slice(&tick.to_le_bytes());
            }
            0xF3 => {
                let check = request
                    .get(cursor..cursor + 6)
                    .context("short Warden memory check")?;
                ensure!(
                    check[0] == 0,
                    "Warden memory check requests an external module"
                );
                let address = u32::from_le_bytes(check[1..5].try_into()?);
                let len = check[5] as usize;
                let bytes = image
                    .read(address, len)
                    .context("Warden memory address unavailable in client image")?;
                results.push(0);
                results.extend_from_slice(bytes);
                cursor += 6;
            }
            0xB2 | 0xBF => {
                request
                    .get(cursor..cursor + 29)
                    .context("short Warden page check")?;
                results.push(0xE9);
                cursor += 29;
            }
            0x71 => {
                let check = request
                    .get(cursor..cursor + 25)
                    .context("short Warden driver check")?;
                ensure!(
                    check[24] > 0 && check[24] as usize <= strings.len(),
                    "invalid Warden driver string index"
                );
                results.push(0xE9);
                cursor += 25;
            }
            0xD9 => {
                request
                    .get(cursor..cursor + 24)
                    .context("short Warden module check")?;
                results.push(0xE9);
                cursor += 24;
            }
            139 => {
                let index = *request.get(cursor).context("short Warden Lua check")? as usize;
                ensure!(
                    index > 0 && index <= strings.len(),
                    "invalid Warden Lua string index"
                );
                results.push(1);
                cursor += 1;
            }
            _ => bail!("unsupported Warden check type 0x{kind:02X}"),
        }
    }
    ensure!(cursor == end, "Warden check request has trailing bytes");
    ensure!(
        results.len() <= u16::MAX as usize,
        "Warden response too large"
    );
    let digest: [u8; 20] = Sha1::digest(&results).into();
    let checksum = digest.chunks_exact(4).fold(0_u32, |sum, chunk| {
        sum ^ u32::from_le_bytes(chunk.try_into().expect("SHA1 chunk has four bytes"))
    });
    let mut response = Vec::with_capacity(results.len() + 7);
    response.push(2);
    response.extend_from_slice(&(results.len() as u16).to_le_bytes());
    response.extend_from_slice(&checksum.to_le_bytes());
    response.extend_from_slice(&results);
    Ok(response)
}

struct PeImage {
    base: u32,
    bytes: Vec<u8>,
}

impl PeImage {
    fn load(path: &Path) -> Result<Self> {
        let file = fs::read(path)
            .with_context(|| format!("read Warden client image {}", path.display()))?;
        let pe = read_u32(&file, 0x3c)? as usize;
        ensure!(
            file.get(pe..pe + 4) == Some(b"PE\0\0"),
            "Warden client image is not PE"
        );
        let sections = read_u16(&file, pe + 6)? as usize;
        let optional = pe + 24;
        ensure!(
            read_u16(&file, optional)? == 0x10b,
            "Warden client image is not 32-bit PE"
        );
        let base = read_u32(&file, optional + 28)?;
        let image_size = read_u32(&file, optional + 56)? as usize;
        ensure!(
            image_size > 0 && image_size <= 64 * 1024 * 1024,
            "invalid PE image size"
        );
        let mut bytes = vec![0; image_size];
        let header_size = read_u32(&file, optional + 60)? as usize;
        ensure!(
            header_size <= file.len() && header_size <= image_size,
            "invalid PE header size"
        );
        bytes[..header_size].copy_from_slice(&file[..header_size]);
        let table = optional + read_u16(&file, pe + 20)? as usize;
        for index in 0..sections {
            let section = table + index * 40;
            let virtual_address = read_u32(&file, section + 12)? as usize;
            let raw_size = read_u32(&file, section + 16)? as usize;
            let raw_offset = read_u32(&file, section + 20)? as usize;
            let source = file
                .get(raw_offset..raw_offset + raw_size)
                .context("PE section outside file")?;
            let destination = bytes
                .get_mut(virtual_address..virtual_address + raw_size)
                .context("PE section outside image")?;
            destination.copy_from_slice(source);
        }
        // WoW 3.3.5a initializes this value from its file sentinel to 4 at runtime.
        // Its adjacent pointer remains unchanged. Only apply the known clean-client
        // state when the image contains the expected sentinel.
        if base == 0x400000 {
            let offset = (0xAC3DAC_u32 - base) as usize;
            if bytes.get(offset..offset + 4) == Some(&[0xFF; 4][..]) {
                bytes[offset..offset + 4].copy_from_slice(&4_u32.to_le_bytes());
            }
        }
        Ok(Self { base, bytes })
    }

    fn read(&self, address: u32, length: usize) -> Option<&[u8]> {
        let offset = address.checked_sub(self.base)? as usize;
        self.bytes.get(offset..offset.checked_add(length)?)
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .context("short PE header")?
            .try_into()?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .context("short PE header")?
            .try_into()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_reply_has_valid_length_checksum_and_memory_bytes() {
        let mut bytes = vec![0; 32];
        bytes[4..7].copy_from_slice(&[0x12, 0x34, 0x56]);
        let image = PeImage {
            base: 0x400000,
            bytes,
        };
        let xor = 0xA7;
        let mut request = vec![0, 0x57 ^ xor, 0xF3 ^ xor, 0];
        request.extend_from_slice(&0x400004_u32.to_le_bytes());
        request.push(3);
        request.push(xor);
        let response = checks_response(&request, &image).unwrap();
        assert_eq!(response[0], 2);
        assert_eq!(
            u16::from_le_bytes([response[1], response[2]]) as usize,
            response.len() - 7
        );
        assert_eq!(&response[response.len() - 4..], &[0, 0x12, 0x34, 0x56]);
        let digest: [u8; 20] = Sha1::digest(&response[7..]).into();
        let expected = digest.chunks_exact(4).fold(0_u32, |sum, chunk| {
            sum ^ u32::from_le_bytes(chunk.try_into().unwrap())
        });
        assert_eq!(
            u32::from_le_bytes(response[3..7].try_into().unwrap()),
            expected
        );
    }

    #[test]
    fn unknown_check_fails_closed() {
        let image = PeImage {
            base: 0,
            bytes: vec![],
        };
        assert!(checks_response(&[0, 0xAA, 0], &image).is_err());
    }

    #[test]
    fn hash_reply_uses_session_cipher_then_switches_to_module_cipher() {
        let key = [0x55; 40];
        let (client_key, server_key) = keys(&key);
        let mut client = WardenClient::new(&key, None).unwrap();
        client.phase = Phase::AwaitHash;

        let mut request = vec![5];
        request.extend_from_slice(&MODULE_SEED);
        Rc4::new(&server_key).apply(&mut request);
        let mut reply = client.handle(&request).unwrap().body.unwrap();
        Rc4::new(&client_key).apply(&mut reply);
        assert_eq!(reply[0], 4);
        assert_eq!(&reply[1..], &CLIENT_HASH);
        assert_eq!(client.phase, Phase::AwaitChecks);

        let mut initialize = vec![3, 1, 2, 3];
        Rc4::new(&SERVER_KEY).apply(&mut initialize);
        let result = client.handle(&initialize).unwrap();
        assert_eq!(result.event, "module initialized");
        assert!(result.body.is_none());
    }
}
