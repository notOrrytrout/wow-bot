use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id_type { ($name:ident) => {
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
    #[serde(transparent)] pub struct $name(pub u64);
    impl $name { pub const fn new(v:u64)->Self{Self(v)} pub const fn get(self)->u64{self.0} }
    impl fmt::Display for $name { fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{write!(f,"{}",self.0)} }
};}
id_type!(AccountId); id_type!(ActionId); id_type!(ActivityId); id_type!(EntityId); id_type!(LaneId);
id_type!(MissionId); id_type!(MovementId); id_type!(RequestId); id_type!(TaskId); id_type!(WorkerId);
