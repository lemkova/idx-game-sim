//! Central limit order book with per-order queues (price-time priority).

use std::collections::{BTreeMap, HashMap, VecDeque};

use crate::types::{Lots, Order, Price, Side};

#[derive(Clone, Copy, Debug)]
pub struct LevelView {
    pub price: Price,
    pub lots: Lots,
    pub freq: usize, // number of resting orders at the level
}

#[derive(Default)]
pub struct OrderBook {
    pub(crate) bids: BTreeMap<Price, VecDeque<Order>>,
    pub(crate) offers: BTreeMap<Price, VecDeque<Order>>,
    pub(crate) index: HashMap<u64, (Side, Price)>,
}

impl OrderBook {
    pub fn best_bid(&self) -> Option<Price> {
        self.bids.keys().next_back().copied()
    }

    pub fn best_offer(&self) -> Option<Price> {
        self.offers.keys().next().copied()
    }

    pub fn insert(&mut self, order: Order) {
        self.index.insert(order.id, (order.side, order.price));
        let map = match order.side {
            Side::Bid => &mut self.bids,
            Side::Offer => &mut self.offers,
        };
        map.entry(order.price).or_default().push_back(order);
    }

    pub fn cancel(&mut self, id: u64) -> Option<Order> {
        let (side, price) = self.index.remove(&id)?;
        let map = match side {
            Side::Bid => &mut self.bids,
            Side::Offer => &mut self.offers,
        };
        let q = map.get_mut(&price)?;
        let pos = q.iter().position(|o| o.id == id)?;
        let order = q.remove(pos);
        if q.is_empty() {
            map.remove(&price);
        }
        order
    }

    pub fn contains(&self, id: u64) -> bool {
        self.index.contains_key(&id)
    }

    pub fn remaining(&self, id: u64) -> Option<Lots> {
        let (side, price) = *self.index.get(&id)?;
        let map = match side {
            Side::Bid => &self.bids,
            Side::Offer => &self.offers,
        };
        map.get(&price)?
            .iter()
            .find(|o| o.id == id)
            .map(|o| o.remaining)
    }

    /// Top-of-book levels, best first.
    pub fn levels(&self, side: Side, depth: usize) -> Vec<LevelView> {
        let view = |(p, q): (&Price, &VecDeque<Order>)| LevelView {
            price: *p,
            lots: q.iter().map(|o| o.remaining).sum(),
            freq: q.len(),
        };
        match side {
            Side::Bid => self.bids.iter().rev().take(depth).map(view).collect(),
            Side::Offer => self.offers.iter().take(depth).map(view).collect(),
        }
    }

    pub fn level_queue(&self, side: Side, price: Price) -> Option<&VecDeque<Order>> {
        match side {
            Side::Bid => self.bids.get(&price),
            Side::Offer => self.offers.get(&price),
        }
    }

    pub fn depth(&self, side: Side) -> usize {
        match side {
            Side::Bid => self.bids.len(),
            Side::Offer => self.offers.len(),
        }
    }
}
