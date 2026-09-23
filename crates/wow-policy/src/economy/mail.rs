use wow_state::Snapshot;
#[derive(Clone,Copy,Debug)]pub struct MailIntent{pub mailbox_generation:u64,pub mail_id:u32}
pub fn current<'a>(state:&'a Snapshot,intent:MailIntent)->Option<&'a wow_state::inventory::MailEntry>{let m=&state.state.inventory.mailbox;(m.generation==intent.mailbox_generation).then(||m.mails.get(&intent.mail_id)).flatten()}
