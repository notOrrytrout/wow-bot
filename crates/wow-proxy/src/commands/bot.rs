use wow_domain::{GroupRole, Mission, MissionId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BotCommand {
    On,
    Off,
    Status,
    Help,
    Mission(Mission),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalBotInput {
    Help,
    Command { character: String, text: String },
}

/// Split a terminal command into its target character and the ordinary
/// target-free command understood by the in-game command parser.
pub fn parse_terminal_input(text: &str) -> Result<Option<TerminalBotInput>, String> {
    let text = text.trim();
    let Some(prefix) = text.get(..4) else {
        return Ok(None);
    };
    if !prefix.eq_ignore_ascii_case(".bot")
        || text
            .as_bytes()
            .get(4)
            .is_some_and(|byte| !byte.is_ascii_whitespace())
    {
        return Ok(None);
    }

    let rest = text[4..].trim();
    if rest.eq_ignore_ascii_case("help") {
        return Ok(Some(TerminalBotInput::Help));
    }
    let Some((verb, after_verb)) = take_word(rest) else {
        return Err("usage: .bot <command> <character> [arguments]".into());
    };
    let Some((character, arguments)) = take_word(after_verb.trim_start()) else {
        return Err(format!("usage: .bot {verb} <character> [arguments]"));
    };
    let character = character.trim();
    if character.is_empty() {
        return Err(format!("usage: .bot {verb} <character> [arguments]"));
    }
    let arguments = arguments.trim_start();
    let command = if arguments.is_empty() {
        format!(".bot {verb}")
    } else {
        format!(".bot {verb} {arguments}")
    };
    Ok(Some(TerminalBotInput::Command {
        character: character.to_owned(),
        text: command,
    }))
}

fn take_word(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    if text.is_empty() {
        return None;
    }
    match text.find(char::is_whitespace) {
        Some(index) => Some((&text[..index], &text[index..])),
        None => Some((text, "")),
    }
}

pub const BOT_HELP_LINES: &[&str] = &[
    ".bot quest | .bot gather \"resource\" | .bot grind \"creature\" | .bot pvp bg | .bot goal \"text\"",
    ".bot party [auto|tank|healer|melee|ranged|support] | .bot raid [auto|tank|healer|melee|ranged|support]",
    ".bot on | .bot off | .bot status | .bot help",
];

pub const HELP_LINES: &[&str] = &[
    BOT_HELP_LINES[0],
    BOT_HELP_LINES[1],
    BOT_HELP_LINES[2],
    ".log start [label] | .log mark [label] | .log status | .log stop | .log help",
    ".log start records packet metadata and limited bodies plus movement positions until .log stop",
];

pub fn parse(text: &str, mission_id: MissionId) -> Result<Option<BotCommand>, String> {
    let trimmed = text.trim();
    if !trimmed.to_ascii_lowercase().starts_with(".bot") {
        return Ok(None);
    }
    let rest = trimmed.get(4..).unwrap_or("").trim();
    if rest.eq_ignore_ascii_case("on") {
        return Ok(Some(BotCommand::On));
    }
    if rest.eq_ignore_ascii_case("off") {
        return Ok(Some(BotCommand::Off));
    }
    if rest.eq_ignore_ascii_case("status") {
        return Ok(Some(BotCommand::Status));
    }
    if rest.eq_ignore_ascii_case("help") {
        return Ok(Some(BotCommand::Help));
    }
    if rest.eq_ignore_ascii_case("quest") {
        return Ok(Some(BotCommand::Mission(Mission::quest(mission_id))));
    }
    if rest.eq_ignore_ascii_case("pvp bg") {
        return Ok(Some(BotCommand::Mission(Mission::battleground(mission_id))));
    }
    if let Some(arg) = command_arg(rest, "gather") {
        return Ok(Some(BotCommand::Mission(Mission::gather(mission_id, arg))));
    }
    if let Some(arg) = command_arg(rest, "grind") {
        return Ok(Some(BotCommand::Mission(Mission::grind(mission_id, arg))));
    }
    if let Some(arg) = command_arg(rest, "goal") {
        return Ok(Some(BotCommand::Mission(Mission::goal(mission_id, arg))));
    }
    if rest.eq_ignore_ascii_case("party") || rest.eq_ignore_ascii_case("party auto") {
        return role_mission(mission_id, "auto", false).map(|m| Some(BotCommand::Mission(m)));
    }
    if let Some(arg) = command_arg(rest, "party") {
        return role_mission(mission_id, arg, false).map(|m| Some(BotCommand::Mission(m)));
    }
    if rest.eq_ignore_ascii_case("raid") || rest.eq_ignore_ascii_case("raid auto") {
        return role_mission(mission_id, "auto", true).map(|m| Some(BotCommand::Mission(m)));
    }
    if let Some(arg) = command_arg(rest, "raid") {
        return role_mission(mission_id, arg, true).map(|m| Some(BotCommand::Mission(m)));
    }
    Ok(None)
}

fn command_arg<'a>(rest: &'a str, command: &str) -> Option<&'a str> {
    let split = rest.find(char::is_whitespace)?;
    let head = &rest[..split];
    let tail = rest[split..].trim_start();
    if !head.eq_ignore_ascii_case(command) {
        return None;
    }
    let value = tail.trim().trim_matches('"').trim();
    (!value.is_empty()).then_some(value)
}

fn parse_role(value: &str) -> Result<GroupRole, String> {
    match value.to_ascii_lowercase().as_str() {
        "auto" => Ok(GroupRole::Auto),
        "tank" => Ok(GroupRole::Tank),
        "healer" | "heal" => Ok(GroupRole::Healer),
        "melee" => Ok(GroupRole::Melee),
        "ranged" => Ok(GroupRole::Ranged),
        "support" => Ok(GroupRole::Support),
        _ => Err(format!("unsupported group role: {value}")),
    }
}
fn role_mission(id: MissionId, value: &str, raid: bool) -> Result<Mission, String> {
    let role = parse_role(value)?;
    let intent = if raid {
        wow_domain::MissionIntent::Raid { role }
    } else {
        wow_domain::MissionIntent::Party { role }
    };
    Ok(Mission {
        id,
        intent,
        permissions: wow_domain::PermissionSet::MOVE
            | wow_domain::PermissionSet::COMBAT
            | wow_domain::PermissionSet::GROUP
            | wow_domain::PermissionSet::LOOT
            | wow_domain::PermissionSet::MAINTENANCE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_domain::MissionIntent;

    #[test]
    fn pvp_bg_and_auto_group_commands_create_expected_missions() {
        let BotCommand::Mission(pvp) = parse(".bot pvp bg", MissionId(9)).unwrap().unwrap() else {
            panic!("mission expected");
        };
        assert_eq!(
            pvp.intent,
            MissionIntent::Battleground { battleground: None }
        );
        let BotCommand::Mission(party) = parse(".bot party", MissionId(10)).unwrap().unwrap()
        else {
            panic!("mission expected");
        };
        assert_eq!(
            party.intent,
            MissionIntent::Party {
                role: GroupRole::Auto
            }
        );
    }
}
