//! Session/game state shared by the GUI frontend and the headless runner.

use std::collections::VecDeque;

use rand::rngs::SmallRng;
use rand::SeedableRng;

use crate::agents::AgentSet;
use crate::idx;
use crate::market::{stock_defs, Market, StockDef};
use crate::player::{margin_cash, OrderStatus, Player, PlayerOrder, INITIAL_CASH};
use crate::types::{
    Lots, OrdType, OwnerId, Price, Side, Trade, BROKER_PLAYER, SHARES_PER_LOT,
};

pub const TICKS_PER_SEC: u64 = 10;
pub const SESSION_SECS: u64 = 600; // 10 minute session
pub const CANDLE_TICKS: u64 = 60 * TICKS_PER_SEC; // 1-minute chart candles
pub const WARMUP_TICKS: u64 = 200;
pub const GLOBAL_TAPE_CAP: usize = 800;
pub const BOOK_DEPTH: usize = 5; // best levels shown in the orderbook panel

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Running,
    Paused,
    Ended,
}

pub struct EntryForm {
    pub side: Side,
    pub otype: OrdType,
    pub price: Price,
    pub lots: Lots,
}

impl EntryForm {
    fn new() -> Self {
        EntryForm {
            side: Side::Bid,
            otype: OrdType::Limit,
            price: 0,
            lots: 10,
        }
    }
}

pub struct App {
    pub defs: [StockDef; 4],
    pub markets: Vec<Market>,
    pub agents: AgentSet,
    pub player: Player,
    pub rng: SmallRng,
    pub seed: u64,
    pub tick: u64, // monotonic; session starts at WARMUP_TICKS
    pub mode: Mode,
    pub selected: usize,
    pub entry: EntryForm,
    pub global_tape: VecDeque<Trade>,
    pub toast: Option<(String, bool)>, // (message, is_error)
}

impl App {
    pub fn new(seed: u64) -> Self {
        let defs = stock_defs();
        let mut rng = SmallRng::seed_from_u64(seed);
        let mut markets: Vec<Market> = defs
            .iter()
            .enumerate()
            .map(|(i, d)| Market::new(i, d))
            .collect();
        let mut agents = AgentSet::new(&defs, &mut rng);
        agents.seed_books(&mut markets, &mut rng);
        for t in 0..WARMUP_TICKS {
            agents.step(&mut markets, t, &mut rng);
        }
        for m in &mut markets {
            m.reset_session_stats();
        }
        App {
            defs,
            markets,
            agents,
            player: Player::new(),
            rng,
            seed,
            tick: WARMUP_TICKS,
            mode: Mode::Running,
            selected: 0,
            entry: EntryForm::new(),
            global_tape: VecDeque::new(),
            toast: None,
        }
    }

    // ---- time helpers ----

    pub fn elapsed_ticks(&self) -> u64 {
        self.tick - WARMUP_TICKS
    }

    pub fn remaining_secs(&self) -> u64 {
        SESSION_SECS.saturating_sub(self.elapsed_ticks() / TICKS_PER_SEC)
    }

    /// Simulated wall clock: session opens at 09:00:00 JKT.
    pub fn sim_clock(&self) -> String {
        let s = 9 * 3600 + self.elapsed_ticks() / TICKS_PER_SEC;
        format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    }

    pub fn clock_of_tick(&self, ts: u64) -> String {
        let s = 9 * 3600 + ts.saturating_sub(WARMUP_TICKS) / TICKS_PER_SEC;
        format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    }

    pub fn lasts(&self) -> [Price; 4] {
        [
            self.markets[0].last,
            self.markets[1].last,
            self.markets[2].last,
            self.markets[3].last,
        ]
    }

    pub fn equity(&self) -> i64 {
        self.player.equity(self.lasts())
    }

    pub fn pnl(&self) -> i64 {
        self.equity() - INITIAL_CASH
    }

    // ---- simulation ----

    pub fn on_tick(&mut self) {
        if self.mode != Mode::Running {
            return;
        }
        self.tick += 1;
        let tick = self.tick;
        self.agents.step(&mut self.markets, tick, &mut self.rng);
        self.drain_events();
        if self.elapsed_ticks() % CANDLE_TICKS == 0 {
            for m in &mut self.markets {
                m.roll_candle();
            }
        }
        if self.elapsed_ticks() >= SESSION_SECS * TICKS_PER_SEC {
            self.end_session();
        }
    }

    /// End the session now (bell or user action): pull every open player
    /// order so the summary is clean, then close the market.
    pub fn end_session(&mut self) {
        if self.mode == Mode::Ended {
            return;
        }
        let open: Vec<(usize, u64)> = self
            .player
            .open_orders()
            .map(|o| (o.stock, o.oid))
            .collect();
        for (stock, oid) in open {
            if self.markets[stock].cancel(oid).is_some() {
                self.player.on_cancel(stock, oid);
            }
        }
        self.mode = Mode::Ended;
    }

    fn drain_events(&mut self) {
        for s in 0..self.markets.len() {
            let evs: Vec<Trade> = self.markets[s].events.drain(..).collect();
            for t in evs {
                if t.buyer == OwnerId::Player {
                    self.player
                        .buy_fill(s, t.buy_oid, t.price, t.lots, t.ts, self.defs[s].leverage);
                }
                if t.seller == OwnerId::Player {
                    self.player.sell_fill(s, t.sell_oid, t.price, t.lots, t.ts);
                }
                self.global_tape.push_back(t);
                if self.global_tape.len() > GLOBAL_TAPE_CAP {
                    self.global_tape.pop_front();
                }
            }
        }
    }

    // ---- GUI-facing actions ----

    pub fn toggle_pause(&mut self) {
        self.mode = match self.mode {
            Mode::Running => Mode::Paused,
            Mode::Paused => Mode::Running,
            Mode::Ended => Mode::Ended,
        };
    }

    pub fn restart(&mut self) {
        *self = App::new(self.seed.wrapping_add(1));
    }

    pub fn select_stock(&mut self, stock: usize) {
        if stock < self.markets.len() && stock != self.selected {
            self.selected = stock;
        }
    }

    /// Open player orders, newest first — the order the UI lists them in.
    pub fn open_orders_newest(&self) -> Vec<&PlayerOrder> {
        let mut v: Vec<&PlayerOrder> = self.player.open_orders().collect();
        v.reverse();
        v
    }

    /// Cancel one of the player's open orders; releases every reservation.
    pub fn cancel_player_order(&mut self, stock: usize, oid: u64) {
        if self.markets[stock].cancel(oid).is_some() {
            self.player.on_cancel(stock, oid);
            self.toast = Some((format!("Order #{oid} cancelled"), false));
        }
    }

    /// Prefill the order ticket: side, limit type, and a sensible price
    /// (given price, else the touch on the opposite side).
    pub fn open_form(&mut self, side: Side, price: Option<Price>) {
        let m = &self.markets[self.selected];
        let default_price = match side {
            Side::Bid => m.best_offer().unwrap_or(m.last),
            Side::Offer => m.best_bid().unwrap_or(m.last),
        };
        self.entry.side = side;
        self.entry.price = price
            .unwrap_or(default_price)
            .clamp(m.lower_bound, m.upper_bound);
        self.entry.lots = self.entry.lots.max(1);
    }


    pub fn toast_ok(&mut self, msg: String) {
        self.toast = Some((msg, false));
    }

    pub fn toast_err(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), true));
    }

    pub fn submit_form(&mut self) {
        let s = self.selected;
        let side = self.entry.side;
        let lots = self.entry.lots;
        let now = self.tick;
        let ticker = self.defs[s].ticker;
        if lots <= 0 {
            self.toast_err("Lots must be at least 1");
            return;
        }

        match self.entry.otype {
            OrdType::Limit => {
                let (lower, upper) = (self.markets[s].lower_bound, self.markets[s].upper_bound);
                let mut price = self.entry.price.clamp(lower, upper);
                price = match side {
                    Side::Bid => idx::snap_down(price),
                    Side::Offer => idx::snap_up(price),
                };
                self.entry.price = price;

                match side {
                    Side::Bid => {
                        let value = lots * SHARES_PER_LOT * price;
                        let lock =
                            margin_cash(value, self.defs[s].leverage) + idx::fee_buy(value);
                        if self.player.cash < lock {
                            self.toast_err(format!(
                                "Insufficient cash: need {}",
                                idx::thousands(lock)
                            ));
                            return;
                        }
                        match self.markets[s].submit_limit(
                            side,
                            price,
                            lots,
                            OwnerId::Player,
                            BROKER_PLAYER,
                            now,
                        ) {
                            Ok(rep) => {
                                self.player.cash -= lock;
                                self.player.reserved_cash += lock;
                                self.player.orders.push(PlayerOrder {
                                    stock: s,
                                    oid: rep.order_id,
                                    side,
                                    otype: OrdType::Limit,
                                    price,
                                    lots,
                                    filled: 0,
                                    status: OrderStatus::Open,
                                    locked: lock,
                                    ts: now,
                                });
                                self.drain_events();
                                self.finish_submit(rep.filled, lots, ticker, "BUY LMT", price);
                            }
                            Err(e) => self.toast_err(e),
                        }
                    }
                    Side::Offer => {
                        if self.player.free_lots(s) < lots {
                            self.toast_err(format!(
                                "Insufficient shares: free {} lot",
                                self.player.free_lots(s)
                            ));
                            return;
                        }
                        match self.markets[s].submit_limit(
                            side,
                            price,
                            lots,
                            OwnerId::Player,
                            BROKER_PLAYER,
                            now,
                        ) {
                            Ok(rep) => {
                                self.player.reserved_lots[s] += lots;
                                self.player.orders.push(PlayerOrder {
                                    stock: s,
                                    oid: rep.order_id,
                                    side,
                                    otype: OrdType::Limit,
                                    price,
                                    lots,
                                    filled: 0,
                                    status: OrderStatus::Open,
                                    locked: 0,
                                    ts: now,
                                });
                                self.drain_events();
                                self.finish_submit(rep.filled, lots, ticker, "SELL LMT", price);
                            }
                            Err(e) => self.toast_err(e),
                        }
                    }
                }
            }
            OrdType::Market => {
                let (fillable, value) = self.markets[s].preview_take(side, lots);
                if fillable == 0 {
                    self.toast_err(match side {
                        Side::Bid => "No offer liquidity",
                        Side::Offer => "No bid liquidity",
                    });
                    return;
                }
                if side == Side::Bid {
                    let cost = margin_cash(value, self.defs[s].leverage) + idx::fee_buy(value);
                    if self.player.cash < cost {
                        self.toast_err(format!("Insufficient cash: need {}", idx::thousands(cost)));
                        return;
                    }
                } else if self.player.free_lots(s) < lots {
                    self.toast_err(format!(
                        "Insufficient shares: free {} lot",
                        self.player.free_lots(s)
                    ));
                    return;
                }
                let res = self.markets[s].submit_market(
                    side,
                    lots,
                    OwnerId::Player,
                    BROKER_PLAYER,
                    now,
                );
                match res {
                    Ok(rep) => {
                        self.player.orders.push(PlayerOrder {
                            stock: s,
                            oid: rep.order_id,
                            side,
                            otype: OrdType::Market,
                            price: 0,
                            lots,
                            filled: 0,
                            status: OrderStatus::Open,
                            locked: 0,
                            ts: now,
                        });
                        self.drain_events();
                        // IOC: whatever did not fill is gone.
                        if let Some(o) = self
                            .player
                            .orders
                            .iter_mut()
                            .rev()
                            .find(|o| o.stock == s && o.oid == rep.order_id)
                        {
                            if o.status == OrderStatus::Open {
                                o.status = if o.filled > 0 {
                                    OrderStatus::Filled
                                } else {
                                    OrderStatus::Cancelled
                                };
                            }
                        }
                        let label = match side {
                            Side::Bid => "BUY MKT",
                            Side::Offer => "SELL MKT",
                        };
                        self.finish_submit(rep.filled, lots, ticker, label, self.markets[s].last);
                    }
                    Err(e) => self.toast_err(e),
                }
            }
        }
    }

    fn finish_submit(
        &mut self,
        filled: Lots,
        lots: Lots,
        ticker: &str,
        label: &str,
        price: Price,
    ) {
        if filled >= lots {
            self.toast_ok(format!(
                "{label} {ticker} {lots} lot filled @ {}",
                idx::thousands(price)
            ));
        } else if filled > 0 {
            self.toast_ok(format!("{label} {ticker} filled {filled}/{lots} lot"));
        } else {
            self.toast_ok(format!(
                "{label} {ticker} {lots} lot @ {} placed",
                idx::thousands(price)
            ));
        }
    }
}
