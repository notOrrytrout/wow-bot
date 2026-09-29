#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LogCommand {
    Start(Option<String>),
    Stop,
    Status,
    Mark(Option<String>),
    Help,
}
pub fn parse(text: &str) -> Option<LogCommand> {
    let t = text.trim();
    let lower = t.to_ascii_lowercase();
    if lower == ".log" || lower == ".log help" {
        Some(LogCommand::Help)
    } else if lower == ".log start" {
        Some(LogCommand::Start(None))
    } else if lower.starts_with(".log start ") {
        Some(LogCommand::Start(Some(
            t[11..].trim().trim_matches('"').to_owned(),
        )))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_accepts_optional_label_and_help_is_local() {
        assert_eq!(parse(".log start"), Some(LogCommand::Start(None)));
        assert_eq!(
            parse(".log start \"raid night\""),
            Some(LogCommand::Start(Some("raid night".into())))
        );
        assert_eq!(parse(".log help"), Some(LogCommand::Help));
    }
}
