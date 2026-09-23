use std::collections::BTreeMap;
#[derive(Clone,Copy,Debug,Eq,PartialEq)]pub enum MemoryMode{Disabled,Optional,Required}
#[derive(Default)]pub struct MemoryStore{entries:BTreeMap<String,String>}
impl MemoryStore{pub fn put(&mut self,k:impl Into<String>,v:impl Into<String>){self.entries.insert(k.into(),v.into());}pub fn get(&self,k:&str)->Option<&str>{self.entries.get(k).map(String::as_str)}pub fn clear(&mut self){self.entries.clear()}}
pub fn require_backend(mode:MemoryMode,available:bool)->Result<(),String>{if mode==MemoryMode::Required&&!available{Err("required memory backend is unavailable".into())}else{Ok(())}}
