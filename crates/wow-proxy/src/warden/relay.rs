use crate::crypt::session::SessionKey;
use sha1::{Digest, Sha1};

#[derive(Debug)]
pub(crate) struct Rc4 {
    state: [u8; 256],
    i: u8,
    j: u8,
}

impl Rc4 {
    pub(crate) fn new(key: &[u8]) -> Self {
        assert!(!key.is_empty(), "RC4 key must not be empty");
        let mut state = [0; 256];
        for (i, byte) in state.iter_mut().enumerate() {
            *byte = i as u8;
        }
        let mut j = 0_u8;
        for i in 0..256 {
            j = j.wrapping_add(state[i]).wrapping_add(key[i % key.len()]);
            state.swap(i, j as usize);
        }
        Self { state, i: 0, j: 0 }
    }

    pub(crate) fn apply(&mut self, data: &mut [u8]) {
        for byte in data {
            self.i = self.i.wrapping_add(1);
            self.j = self.j.wrapping_add(self.state[self.i as usize]);
            self.state.swap(self.i as usize, self.j as usize);
            let index = self.state[self.i as usize].wrapping_add(self.state[self.j as usize]);
            *byte ^= self.state[index as usize];
        }
    }
}

fn sha(parts: &[&[u8]]) -> [u8; 20] {
    let mut hash = Sha1::new();
    for part in parts {
        hash.update(part);
    }
    hash.finalize().into()
}

pub(crate) fn keys(key: &SessionKey) -> ([u8; 16], [u8; 16]) {
    let half = key.len() / 2;
    let a = sha(&[&key[..half]]);
    let b = sha(&[&key[half..]]);
    let mut value = sha(&[&a, &[0; 20], &b]);
    let mut generated = [0; 32];
    generated[..20].copy_from_slice(&value);
    value = sha(&[&a, &value, &b]);
    generated[20..].copy_from_slice(&value[..12]);
    let mut input = [0; 16];
    let mut output = [0; 16];
    input.copy_from_slice(&generated[..16]);
    output.copy_from_slice(&generated[16..]);
    (input, output)
}

pub struct WardenBridge {
    upstream_input: Rc4,
    upstream_output: Rc4,
    downstream_input: Rc4,
    downstream_output: Rc4,
    session_phase: bool,
}

impl WardenBridge {
    pub fn new(upstream: &SessionKey, downstream: &SessionKey) -> Self {
        let (upstream_input, upstream_output) = keys(upstream);
        let (downstream_input, downstream_output) = keys(downstream);
        Self {
            upstream_input: Rc4::new(&upstream_input),
            upstream_output: Rc4::new(&upstream_output),
            downstream_input: Rc4::new(&downstream_input),
            downstream_output: Rc4::new(&downstream_output),
            session_phase: true,
        }
    }

    pub fn server_to_client(&mut self, body: &mut [u8]) {
        if self.session_phase {
            self.upstream_output.apply(body);
            self.downstream_output.apply(body);
        }
    }

    pub fn client_to_server(&mut self, body: &mut [u8]) {
        if self.session_phase {
            self.downstream_input.apply(body);
            let complete = body.first() == Some(&4);
            self.upstream_input.apply(body);
            if complete {
                self.session_phase = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_initial_packets_and_leaves_module_packets_unchanged() {
        let upstream = [0x11; 40];
        let downstream = [0xA7; 40];
        let (up_input, up_output) = keys(&upstream);
        let (down_input, down_output) = keys(&downstream);
        let mut bridge = WardenBridge::new(&upstream, &downstream);

        let mut server = b"server request".to_vec();
        Rc4::new(&up_output).apply(&mut server);
        bridge.server_to_client(&mut server);
        Rc4::new(&down_output).apply(&mut server);
        assert_eq!(server, b"server request");

        let mut client = vec![4, 1, 2, 3];
        Rc4::new(&down_input).apply(&mut client);
        bridge.client_to_server(&mut client);
        Rc4::new(&up_input).apply(&mut client);
        assert_eq!(client, [4, 1, 2, 3]);

        let mut module_packet = vec![9, 8, 7];
        bridge.server_to_client(&mut module_packet);
        assert_eq!(module_packet, [9, 8, 7]);
    }
}
