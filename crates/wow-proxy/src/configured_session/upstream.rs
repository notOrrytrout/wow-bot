use tokio::sync::mpsc;
use wow_domain::GameplayCommand;
pub type UpstreamSender = mpsc::Sender<GameplayCommand>;
