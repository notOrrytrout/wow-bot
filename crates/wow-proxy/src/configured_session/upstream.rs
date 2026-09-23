use wow_domain::GameplayCommand;use tokio::sync::mpsc;pub type UpstreamSender=mpsc::Sender<GameplayCommand>;
