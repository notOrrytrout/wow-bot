use bitflags::bitflags;
use serde::{Deserialize, Serialize};
bitflags! {
    #[derive(Clone,Copy,Debug,Default,Eq,PartialEq,Hash,Serialize,Deserialize)]
    pub struct PermissionSet:u64 {
        const MOVE=1<<0;
        const COMBAT=1<<1;
        const LOOT=1<<2;
        const QUEST=1<<3;
        const GATHER=1<<4;
        const ECONOMY=1<<5;
        const GROUP=1<<6;
        const CHAT=1<<7;
        const ASSET_TRANSFER=1<<8;
        const SERVER_COMMAND=1<<9;
        const MAINTENANCE=1<<10;
        const ALL=u64::MAX;
    }
}
