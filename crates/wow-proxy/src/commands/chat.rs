#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChatFamily { Say, Yell, Party, Raid, Guild, Whisper, Emote, RaidWarning, Battleground, Other }
impl ChatFamily { pub fn supports_local_commands(self)->bool{!matches!(self,Self::Other)} }
pub fn is_local_namespace(text:&str)->bool{let t=text.trim_start().to_ascii_lowercase();t.starts_with(".bot ")||t==".bot"||t.starts_with(".log ")||t==".log"}
