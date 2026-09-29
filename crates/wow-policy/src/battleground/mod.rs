pub mod combat;
pub mod lifecycle;
pub mod objectives;
pub mod vehicles;

/// Return true for WotLK battleground instance maps. Arenas are not included.
pub fn is_wotlk_battleground_map(map_id: u32) -> bool {
    matches!(map_id, 30 | 489 | 529 | 566 | 607 | 628)
}
