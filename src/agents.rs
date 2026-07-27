//! Autonomous market participants for the intraday IDX simulator.

use std::collections::VecDeque;

use rand::rngs::SmallRng;
use rand::Rng;

use crate::idx;
use crate::market::{Market, Persona, StockDef};
use crate::types::{broker_for, AgentKind, Lots, OwnerId, Price, Side};

const LP_TRACKED_CAP: usize = 64;
const RETAIL_TRACKED_CAP: usize = 200;
const SEED_RETAIL_ORDERS: usize = 32;
const DOMESTIC_CHECK_INTERVAL: u64 = 5;
const DOMESTIC_RUNAWAY_TICKS: u64 = 8;

#[derive(Clone, Copy)]
struct LpOrder {
    id: u64,
    side: Side,
    price: Price,
}

struct LpState {
    orders: Vec<LpOrder>,
    refresh_offset: u64,
}

#[derive(Default)]
struct RetailState {
    orders: VecDeque<u64>,
}

enum ForeignState {
    Idle,
    Active {
        side: Side,
        remaining: Lots,
        broker: &'static str,
        next_clip: u64,
    },
}

struct DomesticOrder {
    id: u64,
    price: Price,
}

struct DomesticState {
    broker: &'static str,
    live: Option<DomesticOrder>,
    last_price: Option<Price>,
    reposition: bool,
}

/// All agent state is deliberately held in parallel vectors: one entry per
/// market, with each market borrowed and advanced independently every tick.
pub struct AgentSet {
    personas: Vec<Persona>,
    fair: Vec<f64>,
    lp: Vec<LpState>,
    retail: Vec<RetailState>,
    foreign: Vec<ForeignState>,
    domestic: Vec<DomesticState>,
}

impl AgentSet {
    pub fn new(defs: &[StockDef], rng: &mut SmallRng) -> Self {
        let mut personas = Vec::with_capacity(defs.len());
        let mut fair = Vec::with_capacity(defs.len());
        let mut lp = Vec::with_capacity(defs.len());
        let mut retail = Vec::with_capacity(defs.len());
        let mut foreign = Vec::with_capacity(defs.len());
        let mut domestic = Vec::with_capacity(defs.len());

        for (stock, def) in defs.iter().enumerate() {
            personas.push(def.persona);
            fair.push(def.prev_close as f64);
            lp.push(LpState {
                orders: Vec::new(),
                refresh_offset: stock as u64 % def.persona.lp_refresh_ticks.max(1),
            });
            retail.push(RetailState::default());
            foreign.push(ForeignState::Idle);
            domestic.push(DomesticState {
                broker: broker_for(AgentKind::Domestic, rng),
                live: None,
                last_price: None,
                reposition: true,
            });
        }

        Self {
            personas,
            fair,
            lp,
            retail,
            foreign,
            domestic,
        }
    }

    /// Build the opening book with a full LP ladder, any configured domestic
    /// wall, and dispersed retail interest before warmup begins.
    pub fn seed_books(&mut self, markets: &mut [Market], rng: &mut SmallRng) {
        for s in 0..markets.len() {
            let Some(&persona) = self.personas.get(s) else {
                break;
            };
            let market = &mut markets[s];

            rebuild_lp(
                market,
                persona,
                self.fair[s],
                &mut self.lp[s],
                0,
                rng,
            );
            if let Some(cfg) = persona.domestic {
                place_domestic(
                    market,
                    cfg,
                    &mut self.domestic[s],
                    true,
                    0,
                    rng,
                );
            }
            seed_retail_orders(
                market,
                persona,
                &mut self.retail[s],
                SEED_RETAIL_ORDERS,
                rng,
            );
        }
    }

    pub fn step(&mut self, markets: &mut [Market], tick: u64, rng: &mut SmallRng) {
        for s in 0..markets.len() {
            let Some(&persona) = self.personas.get(s) else {
                break;
            };
            let market = &mut markets[s];

            self.fair[s] = next_fair_value(self.fair[s], market, persona, rng);

            let refresh = persona.lp_refresh_ticks.max(1);
            if tick % refresh == self.lp[s].refresh_offset {
                rebuild_lp(
                    market,
                    persona,
                    self.fair[s],
                    &mut self.lp[s],
                    tick,
                    rng,
                );
            }

            step_retail(
                market,
                persona,
                &mut self.retail[s],
                tick,
                rng,
            );
            step_foreign(
                market,
                persona,
                &mut self.foreign[s],
                self.fair[s],
                tick,
                rng,
            );

            if tick % DOMESTIC_CHECK_INTERVAL == 0 {
                if let Some(cfg) = persona.domestic {
                    step_domestic(market, cfg, &mut self.domestic[s], tick, rng);
                }
            }
        }
    }
}

fn nrand(rng: &mut SmallRng) -> f64 {
    (rng.gen::<f64>() + rng.gen::<f64>() + rng.gen::<f64>()) * 2.0 - 3.0
}

fn next_fair_value(current: f64, market: &Market, persona: Persona, rng: &mut SmallRng) -> f64 {
    let tick = idx::tick_size(market.last) as f64;
    let mut fair = current + nrand(rng) * persona.fair_sigma_ticks * tick;
    // Small permanent impact from traded flow...
    fair += (market.last as f64 - fair) * 0.001;
    // ...but an OU pull back toward the session anchor keeps the walk
    // stationary (no runaway into ARA/ARB every session).
    fair += (market.prev_close as f64 - fair) * 0.002;
    fair.clamp(market.lower_bound as f64, market.upper_bound as f64)
}

fn uniform_lots(rng: &mut SmallRng, min: Lots, max: Lots) -> Lots {
    if min >= max {
        min
    } else {
        rng.gen_range(min..=max)
    }
}

fn uniform_ticks(rng: &mut SmallRng, min: u64, max: u64) -> u64 {
    if min >= max {
        min
    } else {
        rng.gen_range(min..=max)
    }
}

/// A non-round submitted size makes foreign execution visibly distinct from
/// domestic reload walls. The final guard covers the only carry-to-100 case.
fn non_round_lots(rng: &mut SmallRng, min: Lots, max: Lots) -> Lots {
    let mut lots = uniform_lots(rng, min, max) + rng.gen_range(1..=99);
    if lots.rem_euclid(100) == 0 {
        lots += 1;
    }
    lots
}

fn normalize_price(market: &Market, side: Side, raw: Price) -> Option<Price> {
    let clamped = raw.clamp(market.lower_bound, market.upper_bound);
    let price = match side {
        Side::Bid => idx::snap_down(clamped),
        Side::Offer => idx::snap_up(clamped),
    };
    if price >= market.lower_bound && price <= market.upper_bound && idx::valid_price(price) {
        Some(price)
    } else {
        None
    }
}

/// Move farther into the supplied side from an already valid price.
fn step_away(mut price: Price, side: Side, steps: u64) -> Price {
    price = match side {
        Side::Bid => idx::snap_down(price),
        Side::Offer => idx::snap_up(price),
    };
    for _ in 0..steps {
        price = match side {
            Side::Bid => idx::tick_down(price),
            Side::Offer => idx::tick_up(price),
        };
    }
    price
}

fn step_up(mut price: Price, steps: u64) -> Price {
    price = idx::snap_up(price);
    for _ in 0..steps {
        price = idx::tick_up(price);
    }
    price
}

fn step_down(mut price: Price, steps: u64) -> Price {
    price = idx::snap_down(price);
    for _ in 0..steps {
        price = idx::tick_down(price);
    }
    price
}

fn rebuild_lp(
    market: &mut Market,
    persona: Persona,
    fair: f64,
    state: &mut LpState,
    now: u64,
    rng: &mut SmallRng,
) {
    state.orders.retain(|order| {
        market.book.contains(order.id) && market.book.remaining(order.id).unwrap_or(0) > 0
    });
    let fair_price = (fair as Price).clamp(market.lower_bound, market.upper_bound);
    let mut bid0 = idx::snap_down(fair_price);
    let mut off0 = idx::snap_up(fair_price);
    for _ in 0..persona.lp_spread_ticks.max(0) as u64 {
        bid0 = idx::tick_down(bid0);
        off0 = idx::tick_up(off0);
    }
    if let Some(best_offer) = market.best_offer() {
        bid0 = bid0.min(idx::tick_down(best_offer));
    }
    if let Some(best_bid) = market.best_bid() {
        off0 = off0.max(idx::tick_up(best_bid));
    }

    let mut desired = Vec::with_capacity(persona.lp_levels.saturating_mul(2));
    for level in 0..persona.lp_levels as u64 {
        push_lp_target(
            &mut desired,
            market,
            Side::Bid,
            step_down(bid0, level),
        );
        push_lp_target(
            &mut desired,
            market,
            Side::Offer,
            step_up(off0, level),
        );
    }

    let stale: Vec<u64> = state
        .orders
        .iter()
        .filter(|order| {
            !desired
                .iter()
                .any(|&(side, price)| side == order.side && price == order.price)
        })
        .map(|order| order.id)
        .collect();
    for id in stale {
        let _ = market.cancel(id);
    }
    state.orders.retain(|order| {
        market.book.contains(order.id)
            && market.book.remaining(order.id).unwrap_or(0) > 0
            && desired
                .iter()
                .any(|&(side, price)| side == order.side && price == order.price)
    });

    for (side, price) in desired {
        if state
            .orders
            .iter()
            .any(|order| order.side == side && order.price == price)
        {
            continue;
        }
        if state.orders.len() >= LP_TRACKED_CAP {
            break;
        }
        let lots = uniform_lots(rng, persona.lp_lots_min, persona.lp_lots_max);
        if let Ok(report) = market.submit_limit(
            side,
            price,
            lots,
            OwnerId::Agent(AgentKind::Lp),
            "MG",
            now,
        ) {
            if market.book.contains(report.order_id)
                && market.book.remaining(report.order_id).unwrap_or(0) > 0
            {
                state.orders.push(LpOrder {
                    id: report.order_id,
                    side,
                    price,
                });
            }
        }
    }
}

fn push_lp_target(
    desired: &mut Vec<(Side, Price)>,
    market: &Market,
    side: Side,
    raw: Price,
) {
    let Some(price) = normalize_price(market, side, raw) else {
        return;
    };
    if !desired
        .iter()
        .any(|&(known_side, known_price)| known_side == side && known_price == price)
    {
        desired.push((side, price));
    }
}

fn seed_retail_orders(
    market: &mut Market,
    persona: Persona,
    state: &mut RetailState,
    count: usize,
    rng: &mut SmallRng,
) {
    for n in 0..count {
        let side = if n % 2 == 0 { Side::Bid } else { Side::Offer };
        let touch = match side {
            Side::Bid => market.best_bid(),
            Side::Offer => market.best_offer(),
        }
        .unwrap_or(market.last);
        let depth = rng.gen_range(1..=12);
        let raw = step_away(touch, side, depth);
        let Some(price) = normalize_price(market, side, raw) else {
            continue;
        };
        let lots = uniform_lots(rng, persona.retail_lots_min, persona.retail_lots_max);
        let broker = broker_for(AgentKind::Retail, rng);
        if let Ok(report) = market.submit_limit(
            side,
            price,
            lots,
            OwnerId::Agent(AgentKind::Retail),
            broker,
            0,
        ) {
            track_retail_order(market, state, report.order_id);
        }
    }
}

fn step_retail(
    market: &mut Market,
    persona: Persona,
    state: &mut RetailState,
    now: u64,
    rng: &mut SmallRng,
) {
    if !rng.gen_bool(persona.retail_rate.clamp(0.0, 1.0)) {
        return;
    }

    state.orders.retain(|id| market.book.contains(*id));
    if rng.gen_bool(0.10) {
        if !state.orders.is_empty() {
            let index = rng.gen_range(0..state.orders.len());
            if let Some(id) = state.orders.remove(index) {
                let _ = market.cancel(id);
            }
        }
        return;
    }

    let side = if market.last >= market.open {
        if rng.gen_bool(0.58) {
            Side::Bid
        } else {
            Side::Offer
        }
    } else if rng.gen_bool(0.42) {
        Side::Bid
    } else {
        Side::Offer
    };
    let lots = uniform_lots(rng, persona.retail_lots_min, persona.retail_lots_max);
    let broker = broker_for(AgentKind::Retail, rng);

    if rng.gen_bool(persona.retail_aggr.clamp(0.0, 1.0)) {
        let _ = market.submit_market(
            side,
            lots,
            OwnerId::Agent(AgentKind::Retail),
            broker,
            now,
        );
        return;
    }

    let touch = match side {
        Side::Bid => market.best_bid(),
        Side::Offer => market.best_offer(),
    }
    .unwrap_or(market.last);
    // 0/1 ticks account for 70% of postings; 2/3 progressively less often.
    let depth = match rng.gen_range(0..10) {
        0..=3 => 0,
        4..=6 => 1,
        7..=8 => 2,
        _ => 3,
    };
    let raw = step_away(touch, side, depth);
    let Some(price) = normalize_price(market, side, raw) else {
        return;
    };
    if let Ok(report) = market.submit_limit(
        side,
        price,
        lots,
        OwnerId::Agent(AgentKind::Retail),
        broker,
        now,
    ) {
        track_retail_order(market, state, report.order_id);
    }
}

fn track_retail_order(market: &Market, state: &mut RetailState, id: u64) {
    if !market.book.contains(id) {
        return;
    }
    state.orders.push_back(id);
    while state.orders.len() > RETAIL_TRACKED_CAP {
        state.orders.pop_front();
    }
}

fn step_foreign(
    market: &mut Market,
    persona: Persona,
    state: &mut ForeignState,
    fair: f64,
    tick: u64,
    rng: &mut SmallRng,
) {
    match state {
        ForeignState::Idle => {
            if !rng.gen_bool(persona.foreign_start_prob.clamp(0.0, 1.0)) {
                return;
            }
            // Value-driven whale: buy when price trades below fair value,
            // distribute when above. Counters retail momentum instead of
            // stacking on it (which pinned prices at ARA/ARB).
            let preferred = if fair > market.last as f64 {
                Side::Bid
            } else if fair < market.last as f64 {
                Side::Offer
            } else if rng.gen_bool(0.5) {
                Side::Bid
            } else {
                Side::Offer
            };
            let side = if rng.gen_bool(0.55) {
                preferred
            } else if rng.gen_bool(0.5) {
                Side::Bid
            } else {
                Side::Offer
            };
            *state = ForeignState::Active {
                side,
                remaining: non_round_lots(
                    rng,
                    persona.foreign_total_min,
                    persona.foreign_total_max,
                ),
                broker: broker_for(AgentKind::Foreign, rng),
                next_clip: tick
                    .saturating_add(uniform_ticks(
                        rng,
                        persona.foreign_interval_min,
                        persona.foreign_interval_max,
                    )),
            };
        }
        ForeignState::Active {
            side,
            remaining,
            broker,
            next_clip,
        } => {
            if tick < *next_clip {
                return;
            }
            let clip = (*remaining).min(non_round_lots(
                rng,
                persona.foreign_clip_min,
                persona.foreign_clip_max,
            ));
            let touch = match *side {
                Side::Bid => market.best_offer(),
                Side::Offer => market.best_bid(),
            }
            .unwrap_or(market.last);
            let raw = match *side {
                Side::Bid => step_up(touch, rng.gen_range(0..=2)),
                Side::Offer => step_down(touch, rng.gen_range(0..=2)),
            };
            if let Some(price) = normalize_price(market, *side, raw) {
                let _ = market.submit_limit(
                    *side,
                    price,
                    clip,
                    OwnerId::Agent(AgentKind::Foreign),
                    *broker,
                    tick,
                );
            }
            *remaining -= clip;
            if *remaining <= 0 {
                *state = ForeignState::Idle;
            } else {
                *next_clip = tick.saturating_add(uniform_ticks(
                    rng,
                    persona.foreign_interval_min,
                    persona.foreign_interval_max,
                ));
            }
        }
    }
}

fn step_domestic(
    market: &mut Market,
    cfg: crate::market::DomesticCfg,
    state: &mut DomesticState,
    tick: u64,
    rng: &mut SmallRng,
) {
    if let Some(live) = state.live.as_ref() {
        if !market.book.contains(live.id) {
            state.live = None;
        }
    }

    if let Some(live) = state.live.as_ref() {
        let touch = match cfg.side {
            Side::Bid => market.best_bid(),
            Side::Offer => market.best_offer(),
        }
        .unwrap_or(live.price);
        if has_run_away(cfg.side, touch, live.price, DOMESTIC_RUNAWAY_TICKS) {
            let _ = market.cancel(live.id);
            state.live = None;
            state.reposition = true;
        }
        return;
    }

    place_domestic(market, cfg, state, false, tick, rng);
}

fn place_domestic(
    market: &mut Market,
    cfg: crate::market::DomesticCfg,
    state: &mut DomesticState,
    initial_or_reposition: bool,
    now: u64,
    rng: &mut SmallRng,
) {
    let from_touch = initial_or_reposition || state.reposition || state.last_price.is_none();
    let previous = state.last_price.unwrap_or(market.last);
    let raw = if from_touch {
        let touch = match cfg.side {
            Side::Bid => market.best_bid(),
            Side::Offer => market.best_offer(),
        }
        .unwrap_or(market.last);
        step_away(touch, cfg.side, 2)
    } else if rng.gen_bool(cfg.same_level_prob.clamp(0.0, 1.0)) {
        previous
    } else {
        step_away(previous, cfg.side, 1)
    };
    let Some(mut price) = normalize_price(market, cfg.side, raw) else {
        return;
    };

    // A reload remains passive even if the touch crossed its prior wall while
    // the order was being consumed.
    match cfg.side {
        Side::Bid => {
            if let Some(offer) = market.best_offer() {
                price = normalize_price(market, cfg.side, price.min(idx::tick_down(offer)))
                    .unwrap_or(price);
            }
        }
        Side::Offer => {
            if let Some(bid) = market.best_bid() {
                price = normalize_price(market, cfg.side, price.max(idx::tick_up(bid)))
                    .unwrap_or(price);
            }
        }
    }

    state.last_price = Some(price);
    state.reposition = false;
    if let Ok(report) = market.submit_limit(
        cfg.side,
        price,
        cfg.lots,
        OwnerId::Agent(AgentKind::Domestic),
        state.broker,
        now,
    ) {
        if market.book.contains(report.order_id) {
            state.live = Some(DomesticOrder {
                id: report.order_id,
                price,
            });
        }
    }
}

fn has_run_away(side: Side, touch: Price, price: Price, allowed_ticks: u64) -> bool {
    match side {
        Side::Bid if touch > price => touch >= step_up(price, allowed_ticks.saturating_add(1)),
        Side::Offer if touch < price => touch <= step_down(price, allowed_ticks.saturating_add(1)),
        _ => false,
    }
}
