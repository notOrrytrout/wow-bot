use wow_domain::{MovementEpoch, MovementId, TaskId, WorldPosition};
use wow_navigation::Route;
#[derive(Clone, Debug)]
pub struct MovementRuntime {
    pub id: MovementId,
    pub owner: TaskId,
    pub epoch: MovementEpoch,
    pub destination: WorldPosition,
    pub route: Option<Route>,
    pub waypoint: usize,
}
impl MovementRuntime {
    pub fn replace_route(&mut self, route: Route, epoch: MovementEpoch) -> bool {
        if epoch != self.epoch {
            return false;
        }
        self.route = Some(route);
        self.waypoint = 0;
        true
    }
}
