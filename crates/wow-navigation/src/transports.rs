//! Authored, ground-checked transport route data.

use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};
use wow_domain::WorldPosition;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransportRouteCatalog {
    pub schema_version: u32,
    #[serde(default)]
    pub transports: Vec<AuthoredTransport>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoredTransport {
    pub entry: u32,
    pub name: String,
    pub legs: Vec<AuthoredTransportLeg>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoredTransportLeg {
    pub from_stop: u32,
    pub to_stop: u32,
    pub boarding: WorldPosition,
    pub exit: WorldPosition,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ValidatedTransportRoutes {
    legs: BTreeMap<(u32, u32, u32), AuthoredTransportLeg>,
}

impl TransportRouteCatalog {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let body = fs::read_to_string(path).map_err(|error| {
            format!(
                "failed to read transport routes {}: {error}",
                path.display()
            )
        })?;
        let catalog: Self = serde_json::from_str(&body).map_err(|error| {
            format!(
                "failed to parse transport routes {}: {error}",
                path.display()
            )
        })?;
        catalog.validate_shape()?;
        Ok(catalog)
    }

    pub fn validate_shape(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err(format!(
                "unsupported transport route schema version {}",
                self.schema_version
            ));
        }
        let mut entries = std::collections::BTreeSet::new();
        for transport in &self.transports {
            if transport.entry == 0 || !entries.insert(transport.entry) {
                return Err(format!(
                    "transport entry {} is zero or duplicated",
                    transport.entry
                ));
            }
            if transport.name.trim().is_empty() {
                return Err(format!("transport {} has an empty name", transport.entry));
            }
            let mut pairs = std::collections::BTreeSet::new();
            for leg in &transport.legs {
                if leg.from_stop == leg.to_stop || !pairs.insert((leg.from_stop, leg.to_stop)) {
                    return Err(format!(
                        "transport {} has a same-stop or duplicate leg {} -> {}",
                        transport.entry, leg.from_stop, leg.to_stop
                    ));
                }
                for (label, point) in [("boarding", leg.boarding), ("exit", leg.exit)] {
                    if !point.point.is_finite() || !point.orientation.is_finite() {
                        return Err(format!(
                            "transport {} leg {} -> {} has non-finite {label} coordinates",
                            transport.entry, leg.from_stop, leg.to_stop
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Keep only legs whose two authored points pass the movement controller's
    /// MMAP projection and ground-surface checks. Invalid legs fail closed.
    pub fn validate_navigation(
        &self,
        movement: &crate::MovementController,
    ) -> (ValidatedTransportRoutes, Vec<String>) {
        let mut routes = ValidatedTransportRoutes::default();
        let mut rejected = Vec::new();
        if let Err(error) = self.validate_shape() {
            return (routes, vec![error]);
        }
        for transport in &self.transports {
            for leg in &transport.legs {
                let prefix = format!(
                    "transport {} leg {} -> {}",
                    transport.entry, leg.from_stop, leg.to_stop
                );
                let result = movement
                    .validate_grounded_waypoint(leg.boarding)
                    .map_err(|error| format!("{prefix} boarding point failed: {error:?}"))
                    .and_then(|()| {
                        movement
                            .validate_grounded_waypoint(leg.exit)
                            .map_err(|error| format!("{prefix} exit point failed: {error:?}"))
                    });
                match result {
                    Ok(()) => {
                        routes
                            .legs
                            .insert((transport.entry, leg.from_stop, leg.to_stop), leg.clone());
                    }
                    Err(reason) => rejected.push(reason),
                }
            }
        }
        (routes, rejected)
    }
}

impl ValidatedTransportRoutes {
    /// Return the shortest authored map-to-map leg sequence. Map changes are
    /// edges only when a validated authored leg connects their grounded stops.
    pub fn route_for_maps(
        &self,
        from_map: u32,
        to_map: u32,
    ) -> Option<Vec<(u32, AuthoredTransportLeg)>> {
        use std::collections::{BTreeMap, VecDeque};
        if from_map == to_map {
            return Some(Vec::new());
        }
        let mut queue = VecDeque::from([from_map]);
        let mut previous: BTreeMap<u32, (u32, u32, AuthoredTransportLeg)> = BTreeMap::new();
        let mut seen = std::collections::BTreeSet::from([from_map]);
        while let Some(map) = queue.pop_front() {
            for ((entry, _, _), leg) in &self.legs {
                if leg.boarding.map != map || !seen.insert(leg.exit.map) {
                    continue;
                }
                previous.insert(leg.exit.map, (map, *entry, leg.clone()));
                if leg.exit.map == to_map {
                    let mut path = Vec::new();
                    let mut cursor = to_map;
                    while cursor != from_map {
                        let (prior, transport, route_leg) = previous.get(&cursor)?.clone();
                        path.push((transport, route_leg));
                        cursor = prior;
                    }
                    path.reverse();
                    return Some(path);
                }
                queue.push_back(leg.exit.map);
            }
        }
        None
    }

    pub fn legs_for_maps(
        &self,
        from_map: u32,
        to_map: u32,
    ) -> impl Iterator<Item = (u32, &AuthoredTransportLeg)> {
        self.legs.iter().filter_map(move |((entry, _, _), leg)| {
            (leg.boarding.map == from_map && leg.exit.map == to_map).then_some((*entry, leg))
        })
    }

    pub fn leg_for_maps(
        &self,
        transport_entry: u32,
        from_map: u32,
        to_map: u32,
    ) -> Option<&AuthoredTransportLeg> {
        self.legs.iter().find_map(|((entry, _, _), leg)| {
            (*entry == transport_entry && leg.boarding.map == from_map && leg.exit.map == to_map)
                .then_some(leg)
        })
    }

    pub fn leg(
        &self,
        transport_entry: u32,
        from_stop: u32,
        to_stop: u32,
    ) -> Option<&AuthoredTransportLeg> {
        self.legs.get(&(transport_entry, from_stop, to_stop))
    }

    pub fn is_empty(&self) -> bool {
        self.legs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.legs.len()
    }
}
