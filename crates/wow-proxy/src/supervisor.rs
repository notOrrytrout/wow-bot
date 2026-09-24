use tokio::sync::mpsc;
use wow_control_proto::SupervisorCommand;
use wow_domain::LaneId;

#[derive(Clone)]
pub struct SupervisorToken(String);
impl SupervisorToken {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for SupervisorToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SupervisorToken([redacted])")
    }
}

#[derive(Clone, Debug)]
pub struct SupervisorAccess {
    pub endpoint: String,
    pub token: SupervisorToken,
}
impl SupervisorAccess {
    pub fn validate(&self) -> Result<(), String> {
        if self.endpoint.trim().is_empty() {
            return Err("supervisor endpoint is empty".into());
        }
        if self.token.expose().len() < 16 {
            return Err("supervisor authentication token is too short".into());
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct LaneSupervisorClient {
    pub lane: LaneId,
    access: SupervisorAccess,
    tx: mpsc::Sender<SupervisorCommand>,
}
impl LaneSupervisorClient {
    pub fn new(
        lane: LaneId,
        access: SupervisorAccess,
        tx: mpsc::Sender<SupervisorCommand>,
    ) -> Result<Self, String> {
        access.validate()?;
        Ok(Self { lane, access, tx })
    }
    pub async fn send(&self, command: SupervisorCommand) -> Result<(), String> {
        let _token = self.access.token.expose();
        self.tx
            .send(command)
            .await
            .map_err(|_| "supervisor unavailable".into())
    }
}
