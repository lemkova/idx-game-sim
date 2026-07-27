//! Per-stock market: matching engine, session stats, tape, stock personas.

use std::collections::VecDeque;

use crate::book::{LevelView, OrderBook};
use crate::idx;
use crate::types::{Lots, Order, OwnerId, Price, Side, SubmitReport, Trade, SHARES_PER_LOT};

pub const TAPE_CAP: usize = 600;

// ---- Stock personas (consumed by src/agents.rs) ----

#[derive(Clone, Copy, Debug)]
pub struct DomesticCfg {
    pub side: Side,           // which side the whale reloads (Bid = accumulator, Offer = distributor)
    pub lots: Lots,           // round-lot reload size
    pub same_level_prob: f64, // probability to reload at the same price vs stepping one tick
}

#[derive(Clone, Copy, Debug)]
pub struct Persona {
    // Liquidity provider (passive MM)
    pub lp_levels: usize,
    pub lp_lots_min: Lots,
    pub lp_lots_max: Lots,
    pub lp_refresh_ticks: u64,
    pub lp_spread_ticks: i64, // half-spread (in ticks) around fair value
    // Retail flow
    pub retail_rate: f64, // expected retail events per sim tick (100ms)
    pub retail_lots_min: Lots,
    pub retail_lots_max: Lots,
    pub retail_aggr: f64, // probability a retail event takes liquidity
    // Foreign algo whale
    pub foreign_start_prob: f64, // per-tick probability to start a program when idle
    pub foreign_total_min: Lots,
    pub foreign_total_max: Lots,
    pub foreign_clip_min: Lots,
    pub foreign_clip_max: Lots,
    pub foreign_interval_min: u64, // ticks between clips
    pub foreign_interval_max: u64,
    // Domestic whale
    pub domestic: Option<DomesticCfg>,
    // Fair value random walk, stddev per tick expressed in ticks
    pub fair_sigma_ticks: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct StockDef {
    pub ticker: &'static str,
    pub name: &'static str,
    pub prev_close: Price,
    pub persona: Persona,
}

pub fn stock_defs() -> [StockDef; 4] {
    [
        StockDef {
            ticker: "BNKA",
            name: "Bank Nusantara Karya",
            prev_close: 9_750,
            persona: Persona {
                lp_levels: 6,
                lp_lots_min: 200,
                lp_lots_max: 900,
                lp_refresh_ticks: 8,
                lp_spread_ticks: 1,
                retail_rate: 0.18,
                retail_lots_min: 1,
                retail_lots_max: 25,
                retail_aggr: 0.40,
                foreign_start_prob: 0.004,
                foreign_total_min: 2_000,
                foreign_total_max: 9_000,
                foreign_clip_min: 300,
                foreign_clip_max: 1_500,
                foreign_interval_min: 10,
                foreign_interval_max: 30,
                domestic: None,
                fair_sigma_ticks: 0.12,
            },
        },
        StockDef {
            ticker: "TLCO",
            name: "Telekom Cakrawala",
            prev_close: 3_180,
            persona: Persona {
                lp_levels: 5,
                lp_lots_min: 100,
                lp_lots_max: 500,
                lp_refresh_ticks: 8,
                lp_spread_ticks: 1,
                retail_rate: 0.15,
                retail_lots_min: 1,
                retail_lots_max: 30,
                retail_aggr: 0.40,
                foreign_start_prob: 0.0025,
                foreign_total_min: 1_500,
                foreign_total_max: 8_000,
                foreign_clip_min: 200,
                foreign_clip_max: 1_000,
                foreign_interval_min: 15,
                foreign_interval_max: 40,
                domestic: Some(DomesticCfg {
                    side: Side::Bid,
                    lots: 2_500,
                    same_level_prob: 0.50,
                }),
                fair_sigma_ticks: 0.15,
            },
        },
        StockDef {
            ticker: "NIKL",
            name: "Nikel Lautan",
            prev_close: 1_545,
            persona: Persona {
                lp_levels: 5,
                lp_lots_min: 80,
                lp_lots_max: 400,
                lp_refresh_ticks: 8,
                lp_spread_ticks: 1,
                retail_rate: 0.25,
                retail_lots_min: 1,
                retail_lots_max: 40,
                retail_aggr: 0.45,
                foreign_start_prob: 0.003,
                foreign_total_min: 2_000,
                foreign_total_max: 10_000,
                foreign_clip_min: 250,
                foreign_clip_max: 1_200,
                foreign_interval_min: 10,
                foreign_interval_max: 30,
                domestic: Some(DomesticCfg {
                    side: Side::Bid,
                    lots: 5_000,
                    same_level_prob: 0.60,
                }),
                fair_sigma_ticks: 0.25,
            },
        },
        StockDef {
            ticker: "SAWT",
            name: "Sawit Tunas Abadi",
            prev_close: 157,
            persona: Persona {
                lp_levels: 3,
                lp_lots_min: 200,
                lp_lots_max: 900,
                lp_refresh_ticks: 10,
                lp_spread_ticks: 2,
                retail_rate: 0.50,
                retail_lots_min: 5,
                retail_lots_max: 150,
                retail_aggr: 0.50,
                foreign_start_prob: 0.0008,
                foreign_total_min: 4_000,
                foreign_total_max: 18_000,
                foreign_clip_min: 500,
                foreign_clip_max: 2_500,
                foreign_interval_min: 8,
                foreign_interval_max: 20,
                domestic: Some(DomesticCfg {
                    side: Side::Offer,
                    lots: 10_000,
                    same_level_prob: 0.65,
                }),
                fair_sigma_ticks: 0.35,
            },
        },
    ]
}

// ---- Market ----

pub struct Market {
    pub stock: usize,
    pub book: OrderBook,
    pub prev_close: Price,
    pub open: Price,
    pub last: Price,
    pub high: Price,
    pub low: Price,
    pub volume_lots: Lots,
    pub value: i64, // rupiah traded
    pub trade_count: u64,
    pub lower_bound: Price, // ARB
    pub upper_bound: Price, // ARA
    pub tape: VecDeque<Trade>,
    pub events: Vec<Trade>, // drained by the app every tick
    next_order_id: u64,
}

impl Market {
    pub fn new(stock: usize, def: &StockDef) -> Self {
        let (lower_bound, upper_bound) = idx::auto_reject_bounds(def.prev_close);
        Market {
            stock,
            book: OrderBook::default(),
            prev_close: def.prev_close,
            open: def.prev_close,
            last: def.prev_close,
            high: def.prev_close,
            low: def.prev_close,
            volume_lots: 0,
            value: 0,
            trade_count: 0,
            lower_bound,
            upper_bound,
            tape: VecDeque::new(),
            events: Vec::new(),
            next_order_id: 1,
        }
    }

    pub fn best_bid(&self) -> Option<Price> {
        self.book.best_bid()
    }

    pub fn best_offer(&self) -> Option<Price> {
        self.book.best_offer()
    }

    pub fn change(&self) -> i64 {
        self.last - self.prev_close
    }

    pub fn change_pct(&self) -> f64 {
        if self.prev_close == 0 {
            0.0
        } else {
            self.change() as f64 * 100.0 / self.prev_close as f64
        }
    }

    pub fn levels(&self, side: Side, depth: usize) -> Vec<LevelView> {
        self.book.levels(side, depth)
    }

    pub fn submit_limit(
        &mut self,
        side: Side,
        price: Price,
        lots: Lots,
        owner: OwnerId,
        broker: &'static str,
        now: u64,
    ) -> Result<SubmitReport, &'static str> {
        if lots <= 0 {
            return Err("lots must be positive");
        }
        if !idx::valid_price(price) {
            return Err("price off tick grid");
        }
        if price < self.lower_bound || price > self.upper_bound {
            return Err("auto reject (ARA/ARB)");
        }
        let order = Order {
            id: self.alloc_oid(),
            side,
            price,
            remaining: lots,
            owner,
            broker,
            ts: now,
        };
        Ok(self.execute(order, price, true, now))
    }

    /// Market order = IOC bounded by ARA/ARB; unfilled remainder is dropped.
    pub fn submit_market(
        &mut self,
        side: Side,
        lots: Lots,
        owner: OwnerId,
        broker: &'static str,
        now: u64,
    ) -> Result<SubmitReport, &'static str> {
        if lots <= 0 {
            return Err("lots must be positive");
        }
        let limit = match side {
            Side::Bid => self.upper_bound,
            Side::Offer => self.lower_bound,
        };
        let order = Order {
            id: self.alloc_oid(),
            side,
            price: limit,
            remaining: lots,
            owner,
            broker,
            ts: now,
        };
        Ok(self.execute(order, limit, false, now))
    }

    pub fn cancel(&mut self, id: u64) -> Option<Order> {
        self.book.cancel(id)
    }

    /// Walk the opposing side: what would `lots` taken by `taker` fill and cost?
    pub fn preview_take(&self, taker: Side, lots: Lots) -> (Lots, i64) {
        let mut need = lots;
        let mut val: i64 = 0;
        let levels: Vec<(Price, Lots)> = match taker {
            Side::Bid => self
                .book
                .offers
                .iter()
                .map(|(p, q)| (*p, q.iter().map(|o| o.remaining).sum()))
                .collect(),
            Side::Offer => self
                .book
                .bids
                .iter()
                .rev()
                .map(|(p, q)| (*p, q.iter().map(|o| o.remaining).sum()))
                .collect(),
        };
        for (price, avail) in levels {
            if need == 0 {
                break;
            }
            let ex = need.min(avail);
            need -= ex;
            val += ex * SHARES_PER_LOT * price;
        }
        (lots - need, val)
    }

    /// Clear session stats after the warmup phase; the book itself is kept.
    pub fn reset_session_stats(&mut self) {
        self.open = self.last;
        self.high = self.last;
        self.low = self.last;
        self.volume_lots = 0;
        self.value = 0;
        self.trade_count = 0;
        self.tape.clear();
        self.events.clear();
    }

    fn alloc_oid(&mut self) -> u64 {
        let id = self.next_order_id;
        self.next_order_id += 1;
        id
    }

    fn execute(&mut self, mut taker: Order, limit: Price, post: bool, now: u64) -> SubmitReport {
        let mut filled: Lots = 0;
        while taker.remaining > 0 {
            let best = match taker.side {
                Side::Bid => self.book.offers.keys().next().copied(),
                Side::Offer => self.book.bids.keys().next_back().copied(),
            };
            let Some(px) = best else { break };
            let crosses = match taker.side {
                Side::Bid => px <= limit,
                Side::Offer => px >= limit,
            };
            if !crosses {
                break;
            }

            // Fill FIFO against the level queue.
            let mut fills: Vec<(u64, OwnerId, &'static str, Lots)> = Vec::new();
            let mut dead: Vec<u64> = Vec::new();
            let level_empty;
            {
                let q = match taker.side {
                    Side::Bid => self.book.offers.get_mut(&px).expect("level exists"),
                    Side::Offer => self.book.bids.get_mut(&px).expect("level exists"),
                };
                while taker.remaining > 0 {
                    let Some(maker) = q.front_mut() else { break };
                    let ex = taker.remaining.min(maker.remaining);
                    taker.remaining -= ex;
                    maker.remaining -= ex;
                    filled += ex;
                    fills.push((maker.id, maker.owner, maker.broker, ex));
                    if maker.remaining == 0 {
                        dead.push(maker.id);
                        q.pop_front();
                    }
                }
                level_empty = q.is_empty();
            }
            for id in dead {
                self.book.index.remove(&id);
            }
            if level_empty {
                match taker.side {
                    Side::Bid => self.book.offers.remove(&px),
                    Side::Offer => self.book.bids.remove(&px),
                };
            }

            for (mid, mowner, mbroker, ex) in fills {
                let (buyer, buy_broker, buy_oid, seller, sell_broker, sell_oid) = match taker.side {
                    Side::Bid => (taker.owner, taker.broker, taker.id, mowner, mbroker, mid),
                    Side::Offer => (mowner, mbroker, mid, taker.owner, taker.broker, taker.id),
                };
                let t = Trade {
                    stock: self.stock,
                    price: px,
                    lots: ex,
                    aggressor: taker.side,
                    buyer,
                    buy_broker,
                    buy_oid,
                    seller,
                    sell_broker,
                    sell_oid,
                    ts: now,
                };
                self.last = px;
                self.high = self.high.max(px);
                self.low = self.low.min(px);
                self.volume_lots += ex;
                self.value += ex * SHARES_PER_LOT * px;
                self.trade_count += 1;
                self.tape.push_back(t);
                if self.tape.len() > TAPE_CAP {
                    self.tape.pop_front();
                }
                self.events.push(t);
            }
        }

        let order_id = taker.id;
        if post && taker.remaining > 0 {
            self.book.insert(taker);
        }
        SubmitReport { order_id, filled }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AgentKind;

    fn mk() -> Market {
        // NIKL-like: prev close 1545, tick 5
        Market::new(2, &stock_defs()[2])
    }

    const A: OwnerId = OwnerId::Agent(AgentKind::Retail);

    #[test]
    fn price_time_priority_and_partial_fill() {
        let mut m = mk();
        let o1 = m.submit_limit(Side::Offer, 1550, 10, A, "YP", 0).unwrap();
        let o2 = m.submit_limit(Side::Offer, 1550, 20, A, "PD", 1).unwrap();
        let _ = m.submit_limit(Side::Offer, 1555, 30, A, "NI", 2).unwrap();
        // Buy 15 crossing: fills o1 fully (10), o2 partially (5), all @1550.
        let rep = m.submit_limit(Side::Bid, 1555, 15, A, "CC", 3).unwrap();
        assert_eq!(rep.filled, 15);
        assert_eq!(m.last, 1550);
        assert!(!m.book.contains(o1.order_id));
        assert_eq!(m.book.remaining(o2.order_id), Some(15));
        assert_eq!(m.best_offer(), Some(1550));
        assert_eq!(m.volume_lots, 15);
        assert_eq!(m.trade_count, 2);
    }

    #[test]
    fn leftover_posts_to_book() {
        let mut m = mk();
        m.submit_limit(Side::Offer, 1550, 10, A, "YP", 0).unwrap();
        let rep = m.submit_limit(Side::Bid, 1550, 25, A, "PD", 1).unwrap();
        assert_eq!(rep.filled, 10);
        assert_eq!(m.book.remaining(rep.order_id), Some(15));
        assert_eq!(m.best_bid(), Some(1550));
    }

    #[test]
    fn market_order_is_ioc() {
        let mut m = mk();
        m.submit_limit(Side::Offer, 1550, 10, A, "YP", 0).unwrap();
        let rep = m.submit_market(Side::Bid, 50, A, "PD", 1).unwrap();
        assert_eq!(rep.filled, 10);
        assert!(!m.book.contains(rep.order_id)); // remainder dropped
        assert_eq!(m.best_offer(), None);
    }

    #[test]
    fn rejects_bad_tick_and_auto_reject() {
        let mut m = mk();
        assert!(m.submit_limit(Side::Bid, 1547, 1, A, "YP", 0).is_err()); // off grid (tick 5)
        assert!(m.submit_limit(Side::Bid, m.upper_bound + 5, 1, A, "YP", 0).is_err());
        assert!(m.submit_limit(Side::Offer, m.lower_bound - 5, 1, A, "YP", 0).is_err());
    }

    #[test]
    fn cancel_removes_order() {
        let mut m = mk();
        let rep = m.submit_limit(Side::Bid, 1540, 10, A, "YP", 0).unwrap();
        let od = m.cancel(rep.order_id).unwrap();
        assert_eq!(od.remaining, 10);
        assert_eq!(m.best_bid(), None);
        assert!(m.cancel(rep.order_id).is_none());
    }
}
