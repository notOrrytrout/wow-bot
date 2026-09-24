#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RealmEntry {
    pub id: u32,
    pub name: String,
    pub advertised_host: String,
    pub port: u16,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RealmRoute {
    pub id: u32,
    pub name: String,
    pub host: String,
    pub port: u16,
}

pub fn select_realm(
    configured_name: &str,
    configured_host: &str,
    realms: &[RealmEntry],
) -> Result<RealmRoute, String> {
    let realm = realms
        .iter()
        .find(|r| r.name.eq_ignore_ascii_case(configured_name))
        .ok_or_else(|| format!("configured realm {configured_name:?} was not returned upstream"))?;
    Ok(RealmRoute {
        id: realm.id,
        name: realm.name.clone(),
        host: configured_host.to_owned(),
        port: realm.port,
    })
}
