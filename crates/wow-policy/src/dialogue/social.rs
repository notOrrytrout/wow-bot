//! Small, deterministic replies for direct greetings and thanks.
//!
//! The caller supplies live scheduler and ownership gates. The returned
//! command must still pass normal action validation with `PlanOrigin::Dialogue`.

use std::time::Duration;

use wow_domain::GameplayCommand;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SocialChannel {
    Say,
    Party,
    Raid,
    Whisper,
}

#[derive(Clone, Copy, Debug)]
pub struct SocialReplyContext<'a> {
    pub channel: SocialChannel,
    pub sender: Option<&'a str>,
    pub message: &'a str,
    pub bot_name: &'a str,
    pub personality: &'a str,
    pub owned: bool,
    pub logged_in: bool,
    pub alive: bool,
    pub in_combat: bool,
    pub idle: bool,
    pub last_reply_ago: Option<Duration>,
    pub cooldown: Duration,
}

/// Return a chat command only when the live state permits a reply.
pub fn reply(context: SocialReplyContext<'_>) -> Option<GameplayCommand> {
    if !context.owned || !context.logged_in || !context.alive || context.in_combat || !context.idle
    {
        return None;
    }
    if context
        .last_reply_ago
        .is_some_and(|elapsed| elapsed < context.cooldown)
    {
        return None;
    }
    let text = normalize(context.message);
    let is_thanks = is_thanks(&text);
    if !is_thanks && !is_greeting(&text) {
        return None;
    }
    // Avoid replying to general /say traffic unless the speaker addresses us.
    if context.channel == SocialChannel::Say
        && !context
            .message
            .to_ascii_lowercase()
            .contains(&context.bot_name.to_ascii_lowercase())
    {
        return None;
    }
    if context.channel == SocialChannel::Whisper
        && context.sender.is_none_or(|name| name.trim().is_empty())
    {
        return None;
    }
    let channel = match context.channel {
        SocialChannel::Say => 1,
        SocialChannel::Party => 2,
        SocialChannel::Raid => 3,
        SocialChannel::Whisper => 7,
    };
    let response = match (context.personality.to_ascii_lowercase().as_str(), is_thanks) {
        ("formal", true) => "You are welcome.",
        ("formal", false) => "Hello.",
        ("friendly", true) => "no problem!",
        ("friendly", false) => "hey!",
        (_, true) => "np",
        (_, false) => "hey",
    };
    let command = GameplayCommand::Chat {
        channel,
        text: response.to_owned(),
    };
    if context.channel == SocialChannel::Whisper {
        // The current chat command has no recipient field, so it cannot safely
        // answer a private message.
        return None;
    }
    Some(command)
}

fn normalize(message: &str) -> String {
    message
        .trim()
        .trim_matches(|c: char| c.is_ascii_punctuation())
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn is_greeting(text: &str) -> bool {
    matches!(
        text,
        "hi" | "hello" | "hey" | "yo" | "sup" | "hiya" | "howdy"
    ) || text.starts_with("hello ")
        || text.starts_with("hey ")
        || text.starts_with("hi ")
}

fn is_thanks(text: &str) -> bool {
    matches!(text, "thanks" | "thank you" | "ty" | "thx")
        || text.starts_with("thanks ")
        || text.starts_with("thank you ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(message: &'a str) -> SocialReplyContext<'a> {
        SocialReplyContext {
            channel: SocialChannel::Party,
            sender: Some("Alice"),
            message,
            bot_name: "Bot",
            personality: "friendly",
            owned: true,
            logged_in: true,
            alive: true,
            in_combat: false,
            idle: true,
            last_reply_ago: None,
            cooldown: Duration::from_secs(30),
        }
    }

    #[test]
    fn replies_to_greetings_and_thanks_with_chat_commands() {
        assert_eq!(
            reply(ctx("Hello!")),
            Some(GameplayCommand::Chat {
                channel: 2,
                text: "hey!".into()
            })
        );
        assert_eq!(
            reply(ctx("thank you")),
            Some(GameplayCommand::Chat {
                channel: 2,
                text: "no problem!".into()
            })
        );
    }

    #[test]
    fn suppresses_when_not_owned_busy_or_rate_limited() {
        let mut c = ctx("hey");
        c.owned = false;
        assert!(reply(c).is_none());
        let mut c = ctx("hey");
        c.in_combat = true;
        assert!(reply(c).is_none());
        let mut c = ctx("hey");
        c.idle = false;
        assert!(reply(c).is_none());
        let mut c = ctx("hey");
        c.last_reply_ago = Some(Duration::from_secs(5));
        assert!(reply(c).is_none());
    }

    #[test]
    fn suppresses_unaddressed_say_unknown_text_and_unsafe_whispers() {
        let mut c = ctx("hello");
        c.channel = SocialChannel::Say;
        assert!(reply(c).is_none());
        let c = ctx("they are here");
        assert!(reply(c).is_none());
        let mut c = ctx("hi");
        c.channel = SocialChannel::Whisper;
        assert!(reply(c).is_none());
    }
}
