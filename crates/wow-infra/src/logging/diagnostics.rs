use std::{collections::BTreeMap,time::{Duration,Instant}};
#[derive(Clone,Debug)]pub struct DiagnosticEvent{pub target:String,pub message:String}
#[derive(Default)]pub struct DiagnosticThrottle{last:BTreeMap<String,Instant>}
impl DiagnosticThrottle{pub fn should_emit(&mut self,key:&str,now:Instant,min_interval:Duration)->bool{match self.last.get(key){Some(last) if now.duration_since(*last)<min_interval=>false,_=>{self.last.insert(key.to_string(),now);true}}}}
