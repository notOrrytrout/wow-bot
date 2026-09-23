use super::loader::WorldKnowledge;pub fn validate(k:&WorldKnowledge)->Result<(),String>{if k.version.trim().is_empty(){Err("world knowledge version is empty".into())}else{Ok(())}}
