use crate::Route;
use wow_domain::Vec3;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NavigationError { InvalidCoordinate, MissingNavigationData, NoRoute, FloorDiscontinuity, RetryExhausted }

pub trait RoutePlanner: Send + Sync { fn plan(&self, from: Vec3, to: Vec3) -> Result<Route, NavigationError>; }

#[derive(Clone, Debug)]
pub struct DirectPlanner { pub navigation_data_available: bool }
impl Default for DirectPlanner { fn default() -> Self { Self { navigation_data_available: false } } }
impl RoutePlanner for DirectPlanner {
    fn plan(&self, from: Vec3, to: Vec3) -> Result<Route, NavigationError> {
        if !from.is_finite() || !to.is_finite() { return Err(NavigationError::InvalidCoordinate); }
        if !self.navigation_data_available { return Err(NavigationError::MissingNavigationData); }
        Ok(Route::direct(from, to))
    }
}
