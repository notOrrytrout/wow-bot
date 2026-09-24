use wow_domain::LaneId;
#[derive(Clone, Debug)]
pub struct StartupGate {
    pub config_valid: bool,
    pub runtime_data_valid: bool,
    pub private_paths_valid: bool,
    pub supervisor_control_ready: bool,
}
impl StartupGate {
    pub fn ready(&self) -> Result<(), String> {
        if !self.config_valid {
            return Err("configuration validation failed".into());
        }
        if !self.runtime_data_valid {
            return Err("runtime data validation failed".into());
        }
        if !self.private_paths_valid {
            return Err("private path validation failed".into());
        }
        if !self.supervisor_control_ready {
            return Err("supervisor control is not established".into());
        }
        Ok(())
    }
}
pub fn startup_order(mut lanes: Vec<LaneId>) -> Vec<LaneId> {
    lanes.sort();
    lanes
}
