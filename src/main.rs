mod agents;
mod app;
mod book;
mod gui;
mod idx;
mod market;
mod player;
mod types;

use crate::app::{App, TICKS_PER_SEC};
use crate::types::Side;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut seed: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 ^ d.as_secs())
        .unwrap_or(42);
    let mut headless: Option<u64> = None;

    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--seed" => {
                if let Some(v) = it.next().and_then(|v| v.parse().ok()) {
                    seed = v;
                }
            }
            "--headless" => {
                let secs = it.next().and_then(|v| v.parse().ok()).unwrap_or(600);
                headless = Some(secs);
            }
            _ => {}
        }
    }

    if let Some(secs) = headless {
        run_headless(secs, seed);
        return Ok(());
    }

    gui::run(seed)
}

/// Run the simulation without a UI and print market stats — used to verify
/// and tune agent dynamics: `cargo run -- --headless 120 --seed 7`.
fn run_headless(secs: u64, seed: u64) {
    let mut app = App::new(seed);
    println!("headless sim: {secs}s, seed {seed}");
    for _ in 0..secs * TICKS_PER_SEC {
        app.on_tick();
    }
    for (i, m) in app.markets.iter().enumerate() {
        let d = &app.defs[i];
        println!(
            "\n{} {}  prev {}  open {}  last {} ({:+.2}%)  H {} L {}",
            d.ticker,
            d.name,
            idx::thousands(m.prev_close),
            idx::thousands(m.open),
            idx::thousands(m.last),
            m.change_pct(),
            idx::thousands(m.high),
            idx::thousands(m.low),
        );
        println!(
            "  vol {} lot  val {}  trades {}  ARB/ARA {}/{}  depth {}b/{}o",
            idx::thousands(m.volume_lots),
            idx::compact(m.value),
            m.trade_count,
            idx::thousands(m.lower_bound),
            idx::thousands(m.upper_bound),
            m.book.depth(Side::Bid),
            m.book.depth(Side::Offer),
        );
        let bids = m.levels(Side::Bid, 5);
        let offers = m.levels(Side::Offer, 5);
        for r in 0..bids.len().max(offers.len()) {
            let b = bids
                .get(r)
                .map(|l| format!("{:>4}x{:>8} {:>7}", l.freq, l.lots, idx::thousands(l.price)))
                .unwrap_or_else(|| " ".repeat(21));
            let o = offers
                .get(r)
                .map(|l| format!("{:<7} {:>8}x{:<4}", idx::thousands(l.price), l.lots, l.freq))
                .unwrap_or_default();
            println!("  {b} | {o}");
        }
    }
}
