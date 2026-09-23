use wow_state::ProtocolObservation;
/// Preserve an otherwise-unhandled WotLK packet as evidence. Raw observations do not
/// grant gameplay authority; reviewed adapters may later promote specific packet shapes.
pub fn raw(opcode:u32,body:&[u8])->ProtocolObservation{ProtocolObservation::Raw{opcode,body:body.to_vec()}}
