pub trait DialogueProvider:Send+Sync{fn reply(&self,prompt:&str)->Result<String,String>;}
