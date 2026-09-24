use wow_state::Snapshot;
#[derive(Clone, Copy, Debug)]
pub struct AuctionIntent {
    pub query_generation: u64,
    pub listing_id: u64,
    pub max_buyout: u64,
}
pub fn purchase_valid(
    state: &Snapshot,
    intent: AuctionIntent,
    asset_mutation_allowed: bool,
) -> bool {
    if !asset_mutation_allowed {
        return false;
    }
    let a = &state.state.inventory.auction;
    if a.query_generation != intent.query_generation {
        return false;
    }
    a.listings
        .get(&intent.listing_id)
        .is_some_and(|l| l.buyout <= intent.max_buyout && l.buyout <= state.state.inventory.money)
}
