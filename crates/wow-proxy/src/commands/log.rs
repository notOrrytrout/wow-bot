#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LogCommand {
    Start,
    Stop,
    Status,
    Mark(Option<String>),
}
pub fn parse(text: &str) -> Option<LogCommand> {
    let t = text.trim();
    let lower = t.to_ascii_lowercase();
    if lower == ".log start" {
        Some(LogCommand::Start)
    } else if lower == ".log stop" {
        Some(LogCommand::Stop)
    } else if lower == ".log status" {
        Some(LogCommand::Status)
    } else if lower == ".log mark" {
        Some(LogCommand::Mark(None))
    } else if lower.starts_with(".log mark ") {
        Some(LogCommand::Mark(Some(t[10..].trim().to_string())))
    } else {
        None
    }
}
