//! Core market types shared by the whole simulator.

use rand::rngs::SmallRng;
use rand::Rng;

pub type Price = i64; // rupiah
pub type Lots = i64; // 1 lot = 100 shares (IDX round lot)
pub const SHARES_PER_LOT: i64 = 100;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Bid,
    Offer,
}


#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OrdType {
    Limit,
    Market,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentKind {
    Lp,
    Foreign,
    Domestic,
    Retail,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OwnerId {
    Player,
    Agent(AgentKind),
}

#[derive(Clone, Debug)]
pub struct Order {
    pub id: u64,
    pub side: Side,
    pub price: Price,
    pub remaining: Lots, // unfilled portion
    pub owner: OwnerId,
    pub broker: &'static str,
    pub ts: u64, // sim tick when placed
}

#[derive(Clone, Copy, Debug)]
pub struct Trade {
    pub stock: usize,
    pub price: Price,
    pub lots: Lots,
    pub aggressor: Side, // Bid = buyer lifted the offer, Offer = seller hit the bid
    pub buyer: OwnerId,
    pub buy_broker: &'static str,
    pub buy_oid: u64,
    pub seller: OwnerId,
    pub sell_broker: &'static str,
    pub sell_oid: u64,
    pub ts: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct SubmitReport {
    pub order_id: u64,
    pub filled: Lots,
}

// ---- Broker codes shown on the tape / book composition (IDX style 2-char) ----

pub const BROKER_PLAYER: &str = "YU";
pub const BROKERS_LP: &[&str] = &["MG"];
pub const BROKERS_FOREIGN: &[&str] = &["KZ", "BK", "RX", "CS", "ML", "DB"];
pub const BROKERS_DOMESTIC: &[&str] = &["DX", "SQ", "AK", "IF"];
pub const BROKERS_RETAIL: &[&str] = &["YP", "PD", "NI", "XA", "CC", "GR", "AT", "SF", "XL", "OD"];

pub fn broker_for(kind: AgentKind, rng: &mut SmallRng) -> &'static str {
    let pool = match kind {
        AgentKind::Lp => BROKERS_LP,
        AgentKind::Foreign => BROKERS_FOREIGN,
        AgentKind::Domestic => BROKERS_DOMESTIC,
        AgentKind::Retail => BROKERS_RETAIL,
    };
    pool[rng.gen_range(0..pool.len())]
}
