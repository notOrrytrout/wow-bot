use serde::{Deserialize, Serialize};
macro_rules! rev {
    ($name:ident) => {
        #[derive(
            Clone,
            Copy,
            Debug,
            Default,
            Eq,
            PartialEq,
            Ord,
            PartialOrd,
            Hash,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);
        impl $name {
            pub const ZERO: Self = Self(0);
            pub fn next(self) -> Self {
                Self(self.0.wrapping_add(1))
            }
            pub const fn get(self) -> u64 {
                self.0
            }
        }
    };
}
rev!(StateRevision);
rev!(MissionRevision);
rev!(PermissionRevision);
rev!(WorkerGeneration);
rev!(OwnershipGeneration);
rev!(MovementEpoch);
rev!(ActivityGeneration);
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidityStamp {
    pub state: StateRevision,
    pub mission: MissionRevision,
    pub permission: PermissionRevision,
    pub worker: WorkerGeneration,
    pub ownership: OwnershipGeneration,
    pub movement: MovementEpoch,
}
