pub mod client_edge;pub mod upstream_edge;#[derive(Clone,Debug)]pub struct ClientFrame{pub opcode:u32,pub body:Vec<u8>}#[derive(Clone,Debug)]pub struct ServerFrame{pub opcode:u16,pub body:Vec<u8>}
