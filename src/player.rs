//! Player cash/position accounting with IDX fees and open-order reservations.

use crate::idx;
use crate::types::{Lots, OrdType, Price, Side, SHARES_PER_LOT};

pub const INITIAL_CASH: i64 = 100_000_000; // Rp 100 jt

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OrderStatus {
    Open,
    Filled,
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct PlayerOrder {
    pub stock: usize,
    pub oid: u64,
    pub side: Side,
    pub otype: OrdType,
    pub price: Price, // limit price; for market orders the bound used
    pub lots: Lots,
    pub filled: Lots,
    pub status: OrderStatus,
    pub locked: i64,   // cash locked for open limit buys
    pub leverage: i64, // margin multiplier chosen at entry (1 = cash-only)
    pub ts: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct PlayerTrade {
    pub stock: usize,
    pub side: Side,
    pub price: Price,
    pub lots: Lots,
    pub fee: i64,
    pub ts: u64,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Position {
    pub lots: Lots,
    pub avg: f64,      // average price ex-fee
    pub realized: i64, // realized P&L ex-fee
    pub debt: i64,     // margin loan financing this position
}

/// Cash the player must put up for `value` bought at `leverage`; the
/// remainder is financed as margin debt on the position.
pub fn margin_cash(value: i64, leverage: i64) -> i64 {
    let lev = leverage.max(1);
    (value + lev - 1) / lev
}

pub struct Player {
    pub cash: i64,          // free cash
    pub reserved_cash: i64, // locked by open limit buys
    pub positions: [Position; 4],
    pub reserved_lots: [Lots; 4], // locked by open limit sells
    pub orders: Vec<PlayerOrder>,
    pub trades: Vec<PlayerTrade>,
    pub fees: i64,
}

impl Player {
    pub fn new() -> Self {
        Player {
            cash: INITIAL_CASH,
            reserved_cash: 0,
            positions: [Position::default(); 4],
            reserved_lots: [0; 4],
            orders: Vec::new(),
            trades: Vec::new(),
            fees: 0,
        }
    }

    pub fn free_lots(&self, stock: usize) -> Lots {
        self.positions[stock].lots - self.reserved_lots[stock]
    }

    /// Cash + reservations + stock value net of margin debt.
    pub fn equity(&self, lasts: [Price; 4]) -> i64 {
        let stock_val: i64 = self
            .positions
            .iter()
            .zip(lasts)
            .map(|(p, last)| p.lots * SHARES_PER_LOT * last - p.debt)
            .sum();
        self.cash + self.reserved_cash + stock_val
    }

    pub fn open_orders(&self) -> impl Iterator<Item = &PlayerOrder> {
        self.orders.iter().filter(|o| o.status == OrderStatus::Open)
    }

    fn find_order(&mut self, stock: usize, oid: u64) -> Option<&mut PlayerOrder> {
        self.orders
            .iter_mut()
            .rev()
            .find(|o| o.stock == stock && o.oid == oid)
    }

    /// A buy fill hit one of our orders (resting limit or immediate). With
    /// order leverage > 1 only `value/leverage` cash is spent; the rest is
    /// booked as margin debt against the position.
    pub fn buy_fill(&mut self, stock: usize, oid: u64, price: Price, lots: Lots, ts: u64) {
        let value = lots * SHARES_PER_LOT * price;
        let fee = idx::fee_buy(value);

        let mut release = 0;
        let mut leverage = 1;
        if let Some(o) = self.find_order(stock, oid) {
            leverage = o.leverage.max(1);
            o.filled += lots;
            if o.locked > 0 {
                // Re-target the lock to the remaining size at the limit price;
                // the difference goes back to free cash before we pay the fill.
                let rem_val = (o.lots - o.filled) * SHARES_PER_LOT * o.price;
                let target = margin_cash(rem_val, leverage) + idx::fee_buy(rem_val);
                release = (o.locked - target).max(0);
                o.locked -= release;
            }
            if o.filled >= o.lots {
                o.status = OrderStatus::Filled;
            }
        }
        self.reserved_cash -= release;
        self.cash += release;

        let cash_part = margin_cash(value, leverage);
        self.cash -= cash_part + fee;
        self.fees += fee;
        self.positions[stock].debt += value - cash_part;

        let p = &mut self.positions[stock];
        let old_shares = p.lots * SHARES_PER_LOT;
        let new_shares = old_shares + lots * SHARES_PER_LOT;
        p.avg = (p.avg * old_shares as f64 + value as f64) / new_shares as f64;
        p.lots += lots;

        self.trades.push(PlayerTrade {
            stock,
            side: Side::Bid,
            price,
            lots,
            fee,
            ts,
        });
    }

    /// A sell fill hit one of our orders.
    pub fn sell_fill(&mut self, stock: usize, oid: u64, price: Price, lots: Lots, ts: u64) {
        let value = lots * SHARES_PER_LOT * price;
        let fee = idx::fee_sell(value);

        let mut release_lots = 0;
        if let Some(o) = self.find_order(stock, oid) {
            o.filled += lots;
            if o.otype == OrdType::Limit {
                release_lots = lots;
            }
            if o.filled >= o.lots {
                o.status = OrderStatus::Filled;
            }
        }
        self.reserved_lots[stock] -= release_lots;

        // Selling repays margin debt proportionally to the lots sold; the
        // final lot clears the loan exactly (no rounding residue).
        let p = &mut self.positions[stock];
        let repay = if lots >= p.lots {
            p.debt
        } else {
            p.debt * lots / p.lots
        };
        p.debt -= repay;
        self.cash += value - fee - repay;
        self.fees += fee;

        p.realized += ((price as f64 - p.avg) * (lots * SHARES_PER_LOT) as f64).round() as i64;
        p.lots -= lots;
        if p.lots <= 0 {
            p.lots = 0;
            p.avg = 0.0;
        }

        self.trades.push(PlayerTrade {
            stock,
            side: Side::Offer,
            price,
            lots,
            fee,
            ts,
        });
    }

    /// Called after the exchange confirmed the cancel; releases reservations.
    pub fn on_cancel(&mut self, stock: usize, oid: u64) {
        let mut cash_release = 0;
        let mut lot_release = 0;
        if let Some(o) = self.find_order(stock, oid) {
            if o.status == OrderStatus::Open {
                o.status = OrderStatus::Cancelled;
                if o.side == Side::Bid && o.locked > 0 {
                    cash_release = o.locked;
                    o.locked = 0;
                } else if o.side == Side::Offer && o.otype == OrdType::Limit {
                    lot_release = o.lots - o.filled;
                }
            }
        }
        self.reserved_cash -= cash_release;
        self.cash += cash_release;
        self.reserved_lots[stock] -= lot_release;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_buy_lock_fill_and_release() {
        let mut p = Player::new();
        // Lock a 10-lot bid @ 1550: value 1,550,000 + fee 2,325.
        let value = 10 * SHARES_PER_LOT * 1550;
        let lock = value + idx::fee_buy(value);
        p.cash -= lock;
        p.reserved_cash += lock;
        p.orders.push(PlayerOrder {
            stock: 2,
            oid: 7,
            side: Side::Bid,
            otype: OrdType::Limit,
            price: 1550,
            lots: 10,
            filled: 0,
            status: OrderStatus::Open,
            locked: lock,
            leverage: 1,
            ts: 0,
        });

        // Partial fill 4 lots at a better price (1545).
        p.buy_fill(2, 7, 1545, 4, 1);
        assert_eq!(p.positions[2].lots, 4);
        assert!((p.positions[2].avg - 1545.0).abs() < 1e-9);
        assert_eq!(p.orders[0].filled, 4);
        assert_eq!(p.orders[0].status, OrderStatus::Open);

        // Cancel remainder: every reservation must drain.
        p.on_cancel(2, 7);
        assert_eq!(p.reserved_cash, 0);
        let fill_val = 4 * SHARES_PER_LOT * 1545;
        let expect_cash = INITIAL_CASH - fill_val - idx::fee_buy(fill_val);
        assert_eq!(p.cash, expect_cash);
    }

    #[test]
    fn sell_realizes_pnl_and_frees_lots() {
        let mut p = Player::new();
        p.positions[0] = Position {
            lots: 10,
            avg: 1000.0,
            realized: 0,
            debt: 0,
        };
        p.reserved_lots[0] = 10;
        p.orders.push(PlayerOrder {
            stock: 0,
            oid: 3,
            side: Side::Offer,
            otype: OrdType::Limit,
            price: 1100,
            lots: 10,
            filled: 0,
            status: OrderStatus::Open,
            locked: 0,
            leverage: 1,
            ts: 0,
        });
        p.sell_fill(0, 3, 1100, 10, 5);
        assert_eq!(p.positions[0].lots, 0);
        assert_eq!(p.reserved_lots[0], 0);
        assert_eq!(p.positions[0].realized, 100 * 10 * SHARES_PER_LOT); // (1100-1000)*1000 shares
        assert_eq!(p.orders[0].status, OrderStatus::Filled);
        let value = 10 * SHARES_PER_LOT * 1100;
        assert_eq!(p.cash, INITIAL_CASH + value - idx::fee_sell(value));
    }

    #[test]
    fn margin_buy_finances_and_sell_repays() {
        let mut p = Player::new();
        // Market buy 100 lot @ 2,000 at 4x: value 20,000,000, cash part 5,000,000.
        let value = 100 * SHARES_PER_LOT * 2000;
        let fee = idx::fee_buy(value);
        p.orders.push(PlayerOrder {
            stock: 1,
            oid: 9,
            side: Side::Bid,
            otype: OrdType::Market,
            price: 2000,
            lots: 100,
            filled: 0,
            status: OrderStatus::Open,
            locked: 0,
            leverage: 4,
            ts: 0,
        });
        p.buy_fill(1, 9, 2000, 100, 1);
        assert_eq!(p.cash, INITIAL_CASH - value / 4 - fee);
        assert_eq!(p.positions[1].debt, value - value / 4);
        // Flat P&L mark: equity only drops by the fee paid.
        assert_eq!(p.equity([0, 2000, 0, 0]), INITIAL_CASH - fee);

        // Sell half: half the loan is repaid.
        p.orders.push(PlayerOrder {
            stock: 1,
            oid: 10,
            side: Side::Offer,
            otype: OrdType::Market,
            price: 2100,
            lots: 50,
            filled: 0,
            status: OrderStatus::Open,
            locked: 0,
            leverage: 1,
            ts: 2,
        });
        let half_debt = (value - value / 4) / 2;
        p.sell_fill(1, 10, 2100, 50, 3);
        assert_eq!(p.positions[1].debt, half_debt);
        assert_eq!(p.positions[1].lots, 50);

        // Sell the rest: loan fully cleared, no residue.
        p.orders.push(PlayerOrder {
            stock: 1,
            oid: 11,
            side: Side::Offer,
            otype: OrdType::Market,
            price: 2100,
            lots: 50,
            filled: 0,
            status: OrderStatus::Open,
            locked: 0,
            leverage: 1,
            ts: 4,
        });
        p.sell_fill(1, 11, 2100, 50, 5);
        assert_eq!(p.positions[1].debt, 0);
        assert_eq!(p.positions[1].lots, 0);
        // Realized: (2100-2000) * 100 lot * 100 shares.
        assert_eq!(p.positions[1].realized, 100 * 100 * SHARES_PER_LOT);
    }
}
