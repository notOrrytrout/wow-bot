use wow_domain::{Mission,PermissionSet};pub fn permits(m:&Mission,p:PermissionSet)->bool{m.permissions.contains(p)}
