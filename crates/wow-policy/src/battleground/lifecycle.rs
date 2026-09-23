#[derive(Clone,Copy,Debug,Eq,PartialEq)]pub enum BattlegroundStage{Idle,Queued,Invited,Entering,Active,Completed,Cleanup}
pub fn can_accept_invite(stage:BattlegroundStage,authoritative_invite:bool)->bool{stage==BattlegroundStage::Invited&&authoritative_invite}
