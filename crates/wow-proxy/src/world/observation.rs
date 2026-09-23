#[derive(Clone,Debug)]pub struct RawObservation{pub opcode:u32,pub body:Vec<u8>}pub fn raw(opcode:u32,body:Vec<u8>)->RawObservation{RawObservation{opcode,body}}
