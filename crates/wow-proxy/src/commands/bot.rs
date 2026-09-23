use wow_domain::{GroupRole, Mission, MissionId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BotCommand {
    On,
    Off,
    Status,
    Mission(Mission),
}

pub fn parse(text: &str, mission_id: MissionId) -> Result<Option<BotCommand>, String> {
    let trimmed = text.trim();
    if !trimmed.to_ascii_lowercase().starts_with(".bot") { return Ok(None); }
    let rest = trimmed.get(4..).unwrap_or("").trim();
    if rest.eq_ignore_ascii_case("on") { return Ok(Some(BotCommand::On)); }
    if rest.eq_ignore_ascii_case("off") { return Ok(Some(BotCommand::Off)); }
    if rest.eq_ignore_ascii_case("status") { return Ok(Some(BotCommand::Status)); }
    if rest.eq_ignore_ascii_case("quest") { return Ok(Some(BotCommand::Mission(Mission::quest(mission_id)))); }
    if let Some(arg) = command_arg(rest, "gather") { return Ok(Some(BotCommand::Mission(Mission::gather(mission_id, arg)))); }
    if let Some(arg) = command_arg(rest, "grind") { return Ok(Some(BotCommand::Mission(Mission::grind(mission_id, arg)))); }
    if let Some(arg) = command_arg(rest, "goal") { return Ok(Some(BotCommand::Mission(Mission::goal(mission_id, arg)))); }
    if let Some(arg) = command_arg(rest, "party") { return role_mission(mission_id, arg, false).map(|m| Some(BotCommand::Mission(m))); }
    if let Some(arg) = command_arg(rest, "raid") { return role_mission(mission_id, arg, true).map(|m| Some(BotCommand::Mission(m))); }
    Ok(None)
}

fn command_arg<'a>(rest: &'a str, command: &str) -> Option<&'a str> {
    let split = rest.find(char::is_whitespace)?;
    let head = &rest[..split];
    let tail = rest[split..].trim_start();
    if !head.eq_ignore_ascii_case(command) { return None; }
    let value = tail.trim().trim_matches('"').trim();
    (!value.is_empty()).then_some(value)
}

fn parse_role(value: &str) -> Result<GroupRole, String> {
    match value.to_ascii_lowercase().as_str() {
        "tank" => Ok(GroupRole::Tank), "healer" | "heal" => Ok(GroupRole::Healer), "melee" => Ok(GroupRole::Melee),
        "ranged" => Ok(GroupRole::Ranged), "support" => Ok(GroupRole::Support), _ => Err(format!("unsupported group role: {value}")),
    }
}
fn role_mission(id: MissionId, value: &str, raid: bool) -> Result<Mission, String> {
    let role = parse_role(value)?;
    let intent = if raid { wow_domain::MissionIntent::Raid { role } } else { wow_domain::MissionIntent::Party { role } };
    Ok(Mission { id, intent, permissions: wow_domain::PermissionSet::MOVE | wow_domain::PermissionSet::COMBAT | wow_domain::PermissionSet::GROUP | wow_domain::PermissionSet::LOOT | wow_domain::PermissionSet::MAINTENANCE })
}
