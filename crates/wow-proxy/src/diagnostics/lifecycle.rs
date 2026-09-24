use crate::ownership::OwnershipSnapshot;
#[derive(Clone, Debug)]
pub struct LifecycleEvent {
    pub lane: u64,
    pub message: String,
}
pub fn ownership_event(lane: u64, s: OwnershipSnapshot) -> LifecycleEvent {
    LifecycleEvent {
        lane,
        message: format!(
            "mode={:?} phase={:?} attendance={:?} attended={:?} generation={} movement_epoch={}",
            s.mode,
            s.phase,
            s.attendance,
            s.attended_control,
            s.generation.get(),
            s.movement_epoch.get()
        ),
    }
}
