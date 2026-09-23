use std::{collections::BTreeSet,time::{Duration,Instant}};

#[derive(Clone,Debug)]pub enum PlayerEvent{Attached{connection:u64},Detached{connection:u64},Moved{at:Instant},ChannelActivity{until:Instant},Command(String)}

#[derive(Clone,Debug)]
pub struct PlayerPresence {
    connections:BTreeSet<u64>,
    pub last_movement:Option<Instant>,
    pub channel_block_until:Option<Instant>,
    pub idle_resume_armed:bool,
    pub explicit_manual_off:bool,
    pub idle_window:Duration,
}
impl PlayerPresence {
    pub fn new(idle_window:Duration)->Self{Self{connections:BTreeSet::new(),last_movement:None,channel_block_until:None,idle_resume_armed:false,explicit_manual_off:false,idle_window}}
    pub fn attach(&mut self,id:u64){self.connections.insert(id);}
    pub fn detach(&mut self,id:u64)->bool{self.connections.remove(&id);self.connections.is_empty()}
    pub fn count(&self)->usize{self.connections.len()}
    pub fn moved(&mut self,now:Instant){self.last_movement=Some(now);if !self.explicit_manual_off{self.idle_resume_armed=true;}}
    pub fn bot_on(&mut self){self.explicit_manual_off=false;self.idle_resume_armed=true;}
    pub fn bot_off(&mut self){self.explicit_manual_off=true;self.idle_resume_armed=false;}
    pub fn block_channel_until(&mut self,until:Instant){self.channel_block_until=Some(self.channel_block_until.map_or(until,|old|old.max(until)));}
    pub fn resume_deadline(&self)->Option<Instant>{if !self.idle_resume_armed||self.explicit_manual_off{return None}let movement=self.last_movement.map(|t|t+self.idle_window)?;Some(self.channel_block_until.map_or(movement,|c|c.max(movement)))}
    pub fn should_resume(&self,now:Instant)->bool{self.resume_deadline().is_some_and(|deadline|now>=deadline)}
    pub fn resumed(&mut self){self.idle_resume_armed=false;self.channel_block_until=None;}
}
