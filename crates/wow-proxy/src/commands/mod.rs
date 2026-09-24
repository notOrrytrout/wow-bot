pub mod bot;
pub mod chat;
pub mod log;
use wow_domain::MissionId;
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalCommand {
    Bot(bot::BotCommand),
    Log(log::LogCommand),
}
pub fn parse_local(
    text: &str,
    family: chat::ChatFamily,
    configured_account: bool,
    mission_id: MissionId,
) -> Result<Option<LocalCommand>, String> {
    if !configured_account || !family.supports_local_commands() {
        return Ok(None);
    }
    if let Some(c) = log::parse(text) {
        return Ok(Some(LocalCommand::Log(c)));
    }
    if let Some(c) = bot::parse(text, mission_id)? {
        return Ok(Some(LocalCommand::Bot(c)));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_domain::{MissionId, MissionIntent};
    #[test]
    fn recognized_is_local_and_unknown_dot_passes() {
        let c = parse_local(
            ".bot gather \"Copper Vein\"",
            chat::ChatFamily::Party,
            true,
            MissionId(9),
        )
        .unwrap()
        .unwrap();
        match c {
            LocalCommand::Bot(bot::BotCommand::Mission(m)) => assert_eq!(
                m.intent,
                MissionIntent::Gather {
                    resource: "Copper Vein".into()
                }
            ),
            _ => panic!("wrong command"),
        };
        assert!(
            parse_local(
                ".tele stormwind",
                chat::ChatFamily::Say,
                true,
                MissionId(10)
            )
            .unwrap()
            .is_none()
        );
        assert!(
            parse_local(".bot on", chat::ChatFamily::Say, false, MissionId(10))
                .unwrap()
                .is_none()
        );
    }
}
