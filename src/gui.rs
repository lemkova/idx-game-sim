//! Native GUI frontend (eframe/egui) — professional dark trading-terminal look.

use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Button, Color32, CursorIcon, DragValue, FontId, Key, Label, Margin,
    RichText, Sense, Vec2,
};
use egui_extras::{Column, TableBuilder};

use crate::app::{App, Mode, BOOK_DEPTH, SESSION_SECS, TICKS_PER_SEC};
use crate::idx;
use crate::market::{Candle, Market, StockDef};
use crate::player::{margin_cash, OrderStatus, INITIAL_CASH};
use crate::types::{
    AgentKind, Lots, OrdType, OwnerId, Price, Side, Trade, BROKERS_DOMESTIC, BROKERS_FOREIGN,
    BROKERS_LP, BROKER_PLAYER, SHARES_PER_LOT,
};

// ---- palette (mirrors the old terminal theme) ----
const BG: Color32 = Color32::from_rgb(9, 12, 17);
const PANEL: Color32 = Color32::from_rgb(13, 17, 24);
const HDR: Color32 = Color32::from_rgb(18, 24, 33);
const STRIPE: Color32 = Color32::from_rgb(16, 21, 30);
const FG: Color32 = Color32::from_rgb(196, 203, 214);
const DIM: Color32 = Color32::from_rgb(110, 120, 136);
const FOCUS: Color32 = Color32::from_rgb(255, 176, 0);
const GREEN: Color32 = Color32::from_rgb(52, 208, 128);
const RED: Color32 = Color32::from_rgb(240, 82, 92);
const YELLOW: Color32 = Color32::from_rgb(228, 190, 90);
const CYAN: Color32 = Color32::from_rgb(86, 198, 240);
const MAGENTA: Color32 = Color32::from_rgb(206, 130, 240);
const WHITE: Color32 = Color32::from_rgb(232, 236, 242);
const SEL_BG: Color32 = Color32::from_rgb(38, 50, 68);
const BTN_DARK: Color32 = Color32::from_rgb(26, 33, 44);
const GREEN_DARK: Color32 = Color32::from_rgb(16, 84, 52);
const RED_DARK: Color32 = Color32::from_rgb(96, 30, 36);

fn chg_color(chg: i64) -> Color32 {
    if chg > 0 {
        GREEN
    } else if chg < 0 {
        RED
    } else {
        YELLOW
    }
}

fn owner_color(owner: OwnerId) -> Color32 {
    match owner {
        OwnerId::Player => CYAN,
        OwnerId::Agent(AgentKind::Lp) => DIM,
        OwnerId::Agent(AgentKind::Foreign) => YELLOW,
        OwnerId::Agent(AgentKind::Domestic) => MAGENTA,
        OwnerId::Agent(AgentKind::Retail) => FG,
    }
}

/// Color a broker code by the desk behind it — only meaningful when revealed.
fn broker_color(code: &str) -> Color32 {
    if code == BROKER_PLAYER {
        CYAN
    } else if BROKERS_FOREIGN.contains(&code) {
        YELLOW
    } else if BROKERS_DOMESTIC.contains(&code) {
        MAGENTA
    } else if BROKERS_LP.contains(&code) {
        DIM
    } else {
        FG
    }
}

/// IDX investor-type flag: foreign desks are F, everything else trades domestic.
fn investor_type(code: &str) -> &'static str {
    if BROKERS_FOREIGN.contains(&code) {
        "F"
    } else {
        "D"
    }
}

fn side_color(side: Side) -> Color32 {
    match side {
        Side::Bid => GREEN,
        Side::Offer => RED,
    }
}

fn dim(s: impl Into<String>) -> RichText {
    RichText::new(s.into()).color(DIM)
}

fn fg(s: impl Into<String>) -> RichText {
    RichText::new(s.into()).color(FG)
}

fn col(s: impl Into<String>, c: Color32) -> RichText {
    RichText::new(s.into()).color(c)
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(title).color(DIM).strong().size(12.0));
    ui.separator();
}

fn mmss(ts: u64) -> String {
    let s = ts.saturating_sub(crate::app::WARMUP_TICKS) / TICKS_PER_SEC;
    format!("{:02}:{:02}", s / 60, s % 60)
}

pub fn run(seed: u64) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("IDX Intraday Simulator")
            .with_inner_size([1380.0, 880.0])
            .with_min_inner_size([1180.0, 740.0]),
        ..Default::default()
    };
    eframe::run_native(
        "IDX Intraday Simulator",
        options,
        Box::new(move |cc| {
            setup_style(&cc.egui_ctx);
            Ok(Box::new(GuiApp::new(seed)))
        }),
    )
}

fn setup_style(ctx: &egui::Context) {
    ctx.style_mut(|s| {
        s.override_font_id = Some(FontId::monospace(13.0));
        s.spacing.item_spacing = Vec2::new(7.0, 4.0);
        s.spacing.button_padding = Vec2::new(8.0, 3.0);
        s.spacing.scroll.bar_width = 6.0;
        // Labels must not grab clicks (text selection) — it breaks table-row
        // clicking and flips the cursor to an I-beam over every cell.
        s.interaction.selectable_labels = false;
        s.interaction.multi_widget_text_select = false;
    });
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = HDR;
    v.extreme_bg_color = BG;
    v.faint_bg_color = STRIPE;
    v.selection.bg_fill = SEL_BG;
    v.override_text_color = None;
    v.widgets.noninteractive.fg_stroke.color = FG;
    v.widgets.inactive.fg_stroke.color = FG;
    v.widgets.hovered.fg_stroke.color = WHITE;
    v.widgets.active.fg_stroke.color = WHITE;
    v.widgets.inactive.bg_fill = BTN_DARK;
    v.widgets.inactive.weak_bg_fill = BTN_DARK;
    ctx.set_visuals(v);
}

pub struct GuiApp {
    app: App,
    next_tick: Instant,
    ticket_stock: usize,
    ticket_open: bool, // floating order-ticket window
    center_tab: usize, // 0 portfolio · 1 orders · 2 trade history
    queue_view: Option<(usize, Side, Price)>, // (stock, side, price) under inspection
    admin_open: bool,
    admin_balance: String, // balance input buffer for the admin window
    show_brokers: bool,    // admin override: reveal broker codes/investor type intraday
    was_ended: bool,       // last frame's "session ended" state, for edge detection
    post: PostClose,
}

/// Post-close report windows: which are open, per-window stock selection.
#[derive(Default)]
struct PostClose {
    summary: bool,
    broker: bool,
    broker_stock: usize,
    done: bool,
    done_stock: usize,
    market: bool,
    /// Broker aggregation cache for the currently selected stock.
    cache: Option<(usize, Vec<BrokerRow>)>,
}

impl GuiApp {
    fn new(seed: u64) -> Self {
        let mut app = App::new(seed);
        app.open_form(Side::Bid, None); // sensible initial ticket price
        GuiApp {
            app,
            next_tick: Instant::now() + tick_dur(),
            ticket_stock: 0,
            ticket_open: false,
            center_tab: 0,
            queue_view: None,
            admin_open: false,
            admin_balance: String::new(),
            show_brokers: false,
            was_ended: false,
            post: PostClose::default(),
        }
    }

    /// Broker codes / investor types are anonymized intraday (real IDX rules);
    /// visible when the admin toggle is on or after the session closes.
    fn revealed(&self) -> bool {
        self.show_brokers || self.app.mode == Mode::Ended
    }
}

fn tick_dur() -> Duration {
    Duration::from_millis(1000 / TICKS_PER_SEC)
}

impl eframe::App for GuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ---- fixed-rate simulation stepping (with catch-up cap) ----
        let mut steps = 0;
        while Instant::now() >= self.next_tick {
            self.app.on_tick();
            self.next_tick += tick_dur();
            steps += 1;
            if steps >= 40 {
                self.next_tick = Instant::now() + tick_dur();
                break;
            }
        }
        ctx.request_repaint_after(Duration::from_millis(25));

        self.hotkeys(ctx);

        // Edge-detect the close: pop the session summary once, with fresh state.
        let ended = self.app.mode == Mode::Ended;
        if ended && !self.was_ended {
            self.post = PostClose { summary: true, ..PostClose::default() };
        }
        self.was_ended = ended;

        self.toolbar(ctx);
        self.header(ctx);
        self.status_bar(ctx);
        self.left_panel(ctx);
        self.right_panel(ctx);
        self.center_panel(ctx);
        self.ticket_window(ctx);
        self.queue_window(ctx);
        self.admin_window(ctx);

        if ended {
            self.summary_window(ctx);
            self.broker_summary_window(ctx);
            self.done_detail_window(ctx);
            self.market_summary_window(ctx);
        }
    }
}

impl GuiApp {
    fn hotkeys(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        ctx.input(|i| {
            for (key, stock) in [(Key::Num1, 0), (Key::Num2, 1), (Key::Num3, 2), (Key::Num4, 3)] {
                if i.key_pressed(key) {
                    self.app.select_stock(stock);
                }
            }
            if i.key_pressed(Key::B) {
                self.app.open_form(Side::Bid, None);
                self.ticket_open = true;
            }
            if i.key_pressed(Key::S) {
                self.app.open_form(Side::Offer, None);
                self.ticket_open = true;
            }
            if i.key_pressed(Key::P) && self.app.mode != Mode::Ended {
                self.app.toggle_pause();
            }
            if i.key_pressed(Key::N) && self.app.mode == Mode::Ended {
                self.app.restart();
                self.next_tick = Instant::now() + tick_dur();
                self.queue_view = None;
                self.post = PostClose::default();
            }
        });
    }

    fn header(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("header")
            .frame(egui::Frame::default().fill(HDR).inner_margin(Margin::symmetric(10, 6)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let app = &mut self.app;
                    ui.label(RichText::new("IDX INTRADAY SIM").color(WHITE).strong().size(15.0));
                    ui.label(dim("▏"));
                    ui.label(dim("SESI I"));
                    ui.label(fg(format!("{} JKT", app.sim_clock())));
                    ui.label(dim("▏"));
                    ui.label(dim("LEFT"));
                    let rem = app.remaining_secs();
                    ui.label(
                        RichText::new(format!("{:02}:{:02}", rem / 60, rem % 60))
                            .color(FOCUS)
                            .strong()
                            .size(15.0),
                    );
                    ui.label(dim("▏"));
                    ui.label(dim("CASH"));
                    ui.label(fg(idx::thousands(app.player.cash)));
                    ui.label(dim("▏"));
                    ui.label(dim("EQUITY"));
                    ui.label(col(idx::thousands(app.equity()), WHITE));
                    ui.label(dim("▏"));
                    ui.label(dim("P&L"));
                    let pnl = app.pnl();
                    let pct = pnl as f64 * 100.0 / INITIAL_CASH as f64;
                    ui.label(
                        RichText::new(format!("{} ({:+.2}%)", idx::signed_thousands(pnl), pct))
                            .color(chg_color(pnl))
                            .strong(),
                    );
                    ui.label(dim("▏"));
                    let (state, sc) = match app.mode {
                        Mode::Running => ("LIVE", GREEN),
                        Mode::Paused => ("PAUSED", YELLOW),
                        Mode::Ended => ("CLOSED", RED),
                    };
                    ui.label(RichText::new(format!(" {state} ")).color(BG).background_color(sc).strong());
                });
            });
    }

    /// Toolbar under the header: session controls and tools.
    fn toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar")
            .frame(egui::Frame::default().fill(PANEL).inner_margin(Margin::symmetric(10, 4)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if self.app.mode != Mode::Ended {
                        let label = if self.app.mode == Mode::Paused { "Resume" } else { "Pause" };
                        if ui.add(Button::new(fg(label)).fill(BTN_DARK)).clicked() {
                            self.app.toggle_pause();
                        }
                        if ui.add(Button::new(col("End session", RED)).fill(BTN_DARK)).clicked() {
                            self.app.end_session();
                        }
                    } else if ui.add(Button::new(fg("Summary")).fill(BTN_DARK)).clicked() {
                        self.post.summary = true;
                    }
                    if ui.add(Button::new(fg("New session")).fill(BTN_DARK)).clicked() {
                        self.app.restart();
                        self.next_tick = Instant::now() + tick_dur();
                        self.queue_view = None;
                        self.post = PostClose::default();
                    }
                    ui.label(dim("▏"));
                    if ui.add(Button::new(fg("Admin")).fill(BTN_DARK)).clicked() {
                        self.admin_open = !self.admin_open;
                        if self.admin_open {
                            self.admin_balance = self.app.player.cash.to_string();
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(Button::new(fg("Quit")).fill(BTN_DARK)).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                });
            });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .frame(egui::Frame::default().fill(HDR).inner_margin(Margin::symmetric(10, 4)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    match &self.app.toast {
                        Some((msg, is_err)) => {
                            let c = if *is_err { RED } else { GREEN };
                            ui.label(RichText::new(msg).color(c).strong());
                        }
                        None => {
                            ui.label(dim(
                                "Click a book level to prefill the ticket · click a watchlist row to switch stock · keys: 1-4 stock, B/S ticket, P pause",
                            ));
                        }
                    }
                });
            });
    }

    // ---- left: watchlist + portfolio ----

    fn left_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("left")
            .exact_width(300.0)
            .resizable(false)
            .show(ctx, |ui| {
                section(ui, "WATCHLIST");
                let selected = self.app.selected;
                let mut clicked: Option<usize> = None;
                ui.push_id("watchlist", |ui| {
                    TableBuilder::new(ui)
                        .striped(true)
                        .sense(Sense::click())
                        .column(Column::exact(46.0))
                        .column(Column::exact(28.0))
                        .column(Column::exact(64.0))
                        .column(Column::exact(64.0))
                        .column(Column::remainder())
                        .header(18.0, |mut h| {
                            for t in ["TKR", "⚡", "LAST", "CHG%", "VOL"] {
                                h.col(|ui| { ui.label(dim(t)); });
                            }
                        })
                        .body(|body| {
                            body.rows(20.0, self.app.defs.len(), |mut row| {
                                let i = row.index();
                                row.set_selected(i == selected);
                                let d = &self.app.defs[i];
                                let m = &self.app.markets[i];
                                let c = chg_color(m.change());
                                row.col(|ui| { ui.label(col(d.ticker, WHITE).strong()); });
                                row.col(|ui| {
                                    if d.leverage > 1 {
                                        ui.label(col(format!("{}x", d.leverage), CYAN));
                                    } else {
                                        ui.label(dim("·"));
                                    }
                                });
                                row.col(|ui| {
                                    ui.label(col(format!("{:>7}", idx::thousands(m.last)), c));
                                });
                                row.col(|ui| {
                                    ui.label(col(format!("{:>7}", format!("{:+.2}%", m.change_pct())), c));
                                });
                                row.col(|ui| {
                                    ui.label(fg(format!("{:>6}", idx::compact(m.volume_lots))));
                                });
                                if row.response().clicked() {
                                    clicked = Some(i);
                                }
                            });
                        });
                });
                if let Some(i) = clicked {
                    self.app.select_stock(i);
                }
                // Whole-market prints fill the rest of the panel.
                ui.add_space(2.0);
                let rows = tape_rows(
                    &self.app,
                    self.app.global_tape.iter().rev().take(120),
                    self.revealed(),
                );
                draw_tape(ui, "MARKET TRADE", "market_tape", &rows, true);
            });
    }

    // ---- right: order ticket + orders + trades ----

    fn right_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("right")
            .exact_width(360.0)
            .resizable(false)
            .show(ctx, |ui| {
                self.orderbook_panel(ui);
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let w = (ui.available_width() - 8.0) / 2.0;
                    if ui
                        .add_sized([w, 28.0], Button::new(col("BUY", GREEN).strong()).fill(GREEN_DARK))
                        .clicked()
                    {
                        self.app.open_form(Side::Bid, None);
                        self.ticket_open = true;
                    }
                    if ui
                        .add_sized([w, 28.0], Button::new(col("SELL", RED).strong()).fill(RED_DARK))
                        .clicked()
                    {
                        self.app.open_form(Side::Offer, None);
                        self.ticket_open = true;
                    }
                });
                ui.add_space(2.0);
                let s = self.app.selected;
                let rows = tape_rows(
                    &self.app,
                    self.app.markets[s].tape.iter().rev().take(80),
                    self.revealed(),
                );
                draw_tape(ui, "RUNNING TRADE", "running_trade", &rows, false);
            });
    }

    /// Stockbit-style orderbook: stats grid + combined bid/offer depth table.
    fn orderbook_panel(&mut self, ui: &mut egui::Ui) {
        let s = self.app.selected;
        let d = &self.app.defs[s];
        let revealed = self.revealed();
        {
            let m = &self.app.markets[s];
            let chg = m.change();
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(" {} ", d.ticker))
                        .color(WHITE)
                        .background_color(SEL_BG)
                        .strong()
                        .size(14.0),
                );
                if d.leverage > 1 {
                    ui.label(col(format!("⚡{}x", d.leverage), CYAN));
                }
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    let arrow = if chg > 0 { "▲" } else if chg < 0 { "▼" } else { "•" };
                    ui.label(
                        RichText::new(format!(
                            "{} {} ({:+.2}%)",
                            idx::thousands(m.last),
                            arrow,
                            m.change_pct()
                        ))
                        .color(chg_color(chg))
                        .strong()
                        .size(14.0),
                    );
                });
            });
            ui.label(dim(d.name));
            let avg = if m.volume_lots > 0 {
                m.value / (m.volume_lots * SHARES_PER_LOT)
            } else {
                0
            };
            let kv = |ui: &mut egui::Ui, k: &str, v: String, c: Color32| {
                ui.label(dim(k));
                ui.label(col(v, c));
            };
            egui::Grid::new("ob_stats").num_columns(6).spacing([14.0, 2.0]).show(ui, |ui| {
                kv(ui, "Open", idx::thousands(m.open), px_color(m.prev_close, m.open));
                kv(ui, "Prev", idx::thousands(m.prev_close), FG);
                kv(ui, "Lot", idx::compact(m.volume_lots), FG);
                ui.end_row();
                kv(ui, "High", idx::thousands(m.high), px_color(m.prev_close, m.high));
                kv(ui, "ARA", idx::thousands(m.upper_bound), GREEN);
                kv(ui, "Val", idx::compact(m.value), FG);
                ui.end_row();
                kv(ui, "Low", idx::thousands(m.low), px_color(m.prev_close, m.low));
                kv(ui, "ARB", idx::thousands(m.lower_bound), RED);
                kv(ui, "Avg", idx::thousands(avg), FG);
                ui.end_row();
                if revealed {
                    kv(ui, "F Buy", idx::compact(m.f_buy_val), YELLOW);
                    kv(ui, "F Sell", idx::compact(m.f_sell_val), YELLOW);
                } else {
                    // Foreign flow is only published after the closing bell.
                    kv(ui, "F Buy", "—".into(), DIM);
                    kv(ui, "F Sell", "—".into(), DIM);
                }
                kv(ui, "Freq", idx::thousands(m.trade_count as i64), FG);
                ui.end_row();
            });
        }
        ui.separator();

        let prev = self.app.markets[s].prev_close;
        // Fetch the whole book: 5 rows visible, the rest reachable by scroll,
        // and the totals row covers every level, not just the visible ones.
        let bids = self.app.markets[s].levels(Side::Bid, usize::MAX);
        let offers = self.app.markets[s].levels(Side::Offer, usize::MAX);
        let mark = |side: Side, price: Price| -> bool {
            self.app.player.open_orders().any(|o| {
                o.stock == s && o.side == side && o.otype == OrdType::Limit && o.price == price
            })
        };
        // (freq, lots, price text, raw price, color)
        let row_of = |l: &crate::book::LevelView, side: Side| {
            (
                idx::thousands(l.freq as i64),
                idx::thousands(l.lots),
                format!(
                    "{}{}",
                    if mark(side, l.price) { "•" } else { "" },
                    idx::thousands(l.price)
                ),
                l.price,
                px_color(prev, l.price),
            )
        };
        let bid_rows: Vec<_> = bids.iter().map(|l| row_of(l, Side::Bid)).collect();
        let offer_rows: Vec<_> = offers.iter().map(|l| row_of(l, Side::Offer)).collect();
        let nrows = bid_rows.len().max(offer_rows.len()).max(BOOK_DEPTH);

        let mut price_pick: Option<Price> = None;
        let mut queue_pick: Option<(Side, Price)> = None;
        ui.push_id("orderbook", |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .column(Column::exact(36.0))
                .column(Column::exact(64.0))
                .column(Column::exact(52.0))
                .column(Column::exact(52.0))
                .column(Column::exact(64.0))
                .column(Column::remainder())
                .min_scrolled_height(112.0)
                .max_scroll_height(112.0)
                .header(18.0, |mut h| {
                    for t in ["FREQ", "LOT", "BID", "OFFER", "LOT", "FREQ"] {
                        h.col(|ui| { ui.label(dim(t)); });
                    }
                })
                .body(|body| {
                    body.rows(19.0, nrows, |mut row| {
                        let r = row.index();
                        let clickable_lots =
                            |ui: &mut egui::Ui, lots: &str, side: Side, px: Price,
                             queue_pick: &mut Option<(Side, Price)>| {
                                let resp = ui
                                    .add(Label::new(fg(format!("{lots:>7}"))).sense(Sense::click()))
                                    .on_hover_cursor(CursorIcon::PointingHand)
                                    .on_hover_text("view order queue");
                                if resp.clicked() {
                                    *queue_pick = Some((side, px));
                                }
                            };
                        let clickable_price =
                            |ui: &mut egui::Ui, text: &str, c: Color32, px: Price,
                             price_pick: &mut Option<Price>| {
                                let resp = ui
                                    .add(Label::new(col(text, c).strong()).sense(Sense::click()))
                                    .on_hover_cursor(CursorIcon::PointingHand);
                                if resp.clicked() {
                                    *price_pick = Some(px);
                                }
                            };
                        match bid_rows.get(r) {
                            Some((freq, lots, text, px, c)) => {
                                row.col(|ui| { ui.label(dim(format!("{freq:>4}"))); });
                                row.col(|ui| clickable_lots(ui, lots, Side::Bid, *px, &mut queue_pick));
                                row.col(|ui| clickable_price(ui, text, *c, *px, &mut price_pick));
                            }
                            None => {
                                row.col(|_| {});
                                row.col(|_| {});
                                row.col(|_| {});
                            }
                        }
                        match offer_rows.get(r) {
                            Some((freq, lots, text, px, c)) => {
                                row.col(|ui| clickable_price(ui, text, *c, *px, &mut price_pick));
                                row.col(|ui| clickable_lots(ui, lots, Side::Offer, *px, &mut queue_pick));
                                row.col(|ui| { ui.label(dim(freq.clone())); });
                            }
                            None => {
                                row.col(|_| {});
                                row.col(|_| {});
                                row.col(|_| {});
                            }
                        }
                    });
                });
        });
        // Totals: whole-book freq/lots per side around a bid-vs-offer pressure
        // bar; each figure sits under its table column.
        let (bf, bl) = bids.iter().fold((0i64, 0i64), |a, l| (a.0 + l.freq as i64, a.1 + l.lots));
        let (of, ol) = offers.iter().fold((0i64, 0i64), |a, l| (a.0 + l.freq as i64, a.1 + l.lots));
        let gap = ui.spacing().item_spacing.x;
        let cell = |ui: &mut egui::Ui, w: f32, text: RichText| {
            ui.allocate_ui_with_layout(
                Vec2::new(w, 18.0),
                egui::Layout::left_to_right(Align::Center),
                |ui| {
                    ui.label(text);
                },
            );
        };
        ui.horizontal(|ui| {
            cell(ui, 36.0, dim(format!("{:>4}", idx::thousands(bf))));
            cell(ui, 64.0, col(format!("{:>7}", idx::thousands(bl)), GREEN).strong());
            // The bar spans the BID and OFFER price columns.
            let (resp, p) = ui.allocate_painter(Vec2::new(104.0 + gap, 9.0), Sense::hover());
            let rect = resp.rect;
            let split = rect.left() + rect.width() * (bl as f32 / (bl + ol).max(1) as f32);
            p.rect_filled(
                egui::Rect::from_min_max(rect.min, egui::Pos2::new(split, rect.bottom())),
                2.0,
                GREEN,
            );
            p.rect_filled(
                egui::Rect::from_min_max(egui::Pos2::new(split, rect.top()), rect.max),
                2.0,
                Color32::from_rgb(150, 48, 55),
            );
            cell(ui, 64.0, col(format!("{:>7}", idx::thousands(ol)), RED).strong());
            ui.label(dim(idx::thousands(of)));
        });
        if let Some(price) = price_pick {
            self.app.entry.price = price;
        }
        if let Some((side, price)) = queue_pick {
            self.queue_view = Some((s, side, price));
            self.app.entry.price = price;
        }
    }

    /// Floating order ticket (B/S keys or the Buy/Sell buttons open it).
    fn ticket_window(&mut self, ctx: &egui::Context) {
        if !self.ticket_open {
            return;
        }
        let mut open = true;
        let side = self.app.entry.side;
        let title = match side {
            Side::Bid => "ORDER TICKET · BUY",
            Side::Offer => "ORDER TICKET · SELL",
        };
        egui::Window::new(RichText::new(title).color(side_color(side)).strong())
            .id(egui::Id::new("ticket_window"))
            .open(&mut open)
            .default_pos([420.0, 160.0])
            .default_width(340.0)
            .resizable(false)
            .show(ctx, |ui| self.ticket(ui));
        if !open {
            self.ticket_open = false;
        }
    }

    fn ticket(&mut self, ui: &mut egui::Ui) {
        let s = self.app.selected;
        // Re-price the ticket when the player switches stock (or after a
        // restart) so a stale price never carries across tick-size bands.
        if self.ticket_stock != s || self.app.entry.price == 0 {
            let side = self.app.entry.side;
            self.app.open_form(side, None);
            self.ticket_stock = s;
        }
        let ticker = self.app.defs[s].ticker;
        let name = self.app.defs[s].name;
        let lev = self.app.defs[s].leverage;
        section(ui, "ORDER TICKET");
        ui.horizontal(|ui| {
            ui.label(col(ticker, WHITE).strong().size(14.0));
            ui.label(dim(name));
            if lev > 1 {
                ui.label(col(format!("⚡{lev}x margin"), CYAN));
            } else {
                ui.label(dim("cash only"));
            }
        });

        let enabled = self.app.mode != Mode::Ended;
        ui.add_enabled_ui(enabled, |ui| {
            // side
            ui.horizontal(|ui| {
                let side = self.app.entry.side;
                let w = (ui.available_width() - 8.0) / 2.0;
                let (buy_fill, buy_txt) =
                    if side == Side::Bid { (GREEN, WHITE) } else { (BTN_DARK, DIM) };
                let (sell_fill, sell_txt) =
                    if side == Side::Offer { (RED, WHITE) } else { (BTN_DARK, DIM) };
                if ui
                    .add_sized([w, 28.0], Button::new(col("Buy", buy_txt).strong()).fill(buy_fill))
                    .clicked()
                {
                    self.app.open_form(Side::Bid, None);
                }
                if ui
                    .add_sized([w, 28.0], Button::new(col("Sell", sell_txt).strong()).fill(sell_fill))
                    .clicked()
                {
                    self.app.open_form(Side::Offer, None);
                }
            });
            // type
            ui.horizontal(|ui| {
                ui.label(dim("Type "));
                ui.selectable_value(&mut self.app.entry.otype, OrdType::Limit, "LIMIT");
                ui.selectable_value(&mut self.app.entry.otype, OrdType::Market, "MARKET");
            });
            // price
            let (lower, upper, best_bid, best_offer) = {
                let m = &self.app.markets[s];
                (m.lower_bound, m.upper_bound, m.best_bid(), m.best_offer())
            };
            match self.app.entry.otype {
                OrdType::Limit => {
                    ui.horizontal(|ui| {
                        ui.label(dim("Price"));
                        if ui.button(fg("◀")).clicked() {
                            self.app.entry.price =
                                idx::tick_down(idx::snap_up(self.app.entry.price)).max(lower);
                        }
                        let speed = idx::tick_size(self.app.entry.price) as f64;
                        ui.add(
                            DragValue::new(&mut self.app.entry.price)
                                .range(lower..=upper)
                                .speed(speed)
                                .custom_formatter(|v, _| idx::thousands(v as i64))
                                .custom_parser(|s| s.replace([',', '.'], "").parse::<f64>().ok()),
                        );
                        if ui.button(fg("▶")).clicked() {
                            self.app.entry.price =
                                idx::tick_up(idx::snap_down(self.app.entry.price)).min(upper);
                        }
                        ui.label(dim(format!("tick {}", idx::tick_size(self.app.entry.price))));
                        if let Some(b) = best_bid {
                            if ui.button(fg("@Bid")).clicked() {
                                self.app.entry.price = b;
                            }
                        }
                        if let Some(o) = best_offer {
                            if ui.button(fg("@Offer")).clicked() {
                                self.app.entry.price = o;
                            }
                        }
                    });
                }
                OrdType::Market => {
                    let (fillable, value) =
                        self.app.markets[s].preview_take(self.app.entry.side, self.app.entry.lots);
                    let avg = if fillable > 0 { value / (fillable * SHARES_PER_LOT) } else { 0 };
                    ui.horizontal(|ui| {
                        ui.label(dim("Price"));
                        ui.label(col("MKT", YELLOW).strong());
                        ui.label(dim(format!(
                            "est avg {} · {} lot avail",
                            idx::thousands(avg),
                            idx::thousands(fillable)
                        )));
                    });
                }
            }
            // lots
            ui.horizontal(|ui| {
                ui.label(dim("Lots "));
                ui.add(DragValue::new(&mut self.app.entry.lots).range(1..=1_000_000).speed(1.0));
                for step in [10i64, 100] {
                    if ui.button(fg(format!("+{step}"))).clicked() {
                        self.app.entry.lots += step;
                    }
                }
                if ui.button(fg("min")).clicked() {
                    self.app.entry.lots = 1;
                }
            });
            // value + fee
            let e = &self.app.entry;
            let (value, fee) = match e.otype {
                OrdType::Limit => {
                    let v = e.lots * SHARES_PER_LOT * e.price;
                    (v, match e.side {
                        Side::Bid => idx::fee_buy(v),
                        Side::Offer => idx::fee_sell(v),
                    })
                }
                OrdType::Market => {
                    let (_, v) = self.app.markets[s].preview_take(e.side, e.lots);
                    (v, match e.side {
                        Side::Bid => idx::fee_buy(v),
                        Side::Offer => idx::fee_sell(v),
                    })
                }
            };
            ui.horizontal(|ui| {
                ui.label(dim("Value"));
                ui.label(col(idx::thousands(value), WHITE));
                ui.label(dim(format!("fee {}", idx::thousands(fee))));
                if e.side == Side::Bid && lev > 1 {
                    // Margin: only value/leverage of cash is committed up front.
                    ui.label(col(
                        format!("cash {}", idx::thousands(margin_cash(value, lev) + fee)),
                        CYAN,
                    ));
                }
            });
            // submit
            let side = self.app.entry.side;
            let (label, fill) = match side {
                Side::Bid => (format!("Buy {ticker}"), GREEN),
                Side::Offer => (format!("Sell {ticker}"), RED),
            };
            if ui
                .add_sized(
                    [ui.available_width(), 32.0],
                    Button::new(col(label, WHITE).strong()).fill(fill),
                )
                .clicked()
            {
                self.app.submit_form();
            }
        });
    }

    fn orders_table(&mut self, ui: &mut egui::Ui) {
        struct Row {
            time: String,
            tkr: &'static str,
            side: Side,
            typ: &'static str,
            price: String,
            lots: String,
            filled: String,
            stat: &'static str,
            open: bool,
            stock: usize,
            oid: u64,
        }
        let open_count = self.app.player.open_orders().count();
        let rows: Vec<Row> = {
            let open = self.app.open_orders_newest();
            let closed = self
                .app
                .player
                .orders
                .iter()
                .filter(|o| o.status != OrderStatus::Open)
                .rev();
            open.into_iter()
                .chain(closed)
                .take(60)
                .map(|o| Row {
                    time: mmss(o.ts),
                    tkr: self.app.defs[o.stock].ticker,
                    side: o.side,
                    typ: match o.otype {
                        OrdType::Limit => "LMT",
                        OrdType::Market => "MKT",
                    },
                    price: match o.otype {
                        OrdType::Limit => idx::thousands(o.price),
                        OrdType::Market => "MKT".into(),
                    },
                    lots: idx::thousands(o.lots),
                    filled: idx::thousands(o.filled),
                    stat: match o.status {
                        OrderStatus::Open => "OPEN",
                        OrderStatus::Filled => {
                            if o.filled < o.lots {
                                "PART"
                            } else {
                                "FILL"
                            }
                        }
                        OrderStatus::Cancelled => {
                            if o.filled > 0 {
                                "PART"
                            } else {
                                "CXL"
                            }
                        }
                    },
                    open: o.status == OrderStatus::Open,
                    stock: o.stock,
                    oid: o.oid,
                })
                .collect()
        };

        section(ui, &format!("MY ORDERS · {open_count} open"));
        let mut cancel: Option<(usize, u64)> = None;
        ui.push_id("orders", |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .column(Column::exact(44.0))
                .column(Column::exact(40.0))
                .column(Column::exact(16.0))
                .column(Column::exact(30.0))
                .column(Column::exact(56.0))
                .column(Column::exact(44.0))
                .column(Column::exact(40.0))
                .column(Column::exact(38.0))
                .column(Column::remainder())
                .min_scrolled_height(80.0)
                .max_scroll_height(190.0)
                .header(18.0, |mut h| {
                    for t in ["TIME", "TKR", "S", "TYP", "PRICE", "LOTS", "FILL", "STAT", ""] {
                        h.col(|ui| { ui.label(dim(t)); });
                    }
                })
                .body(|body| {
                    body.rows(20.0, rows.len(), |mut row| {
                        let r = &rows[row.index()];
                        let base = if r.open { side_color(r.side) } else { DIM };
                        row.col(|ui| { ui.label(col(r.time.clone(), DIM)); });
                        row.col(|ui| { ui.label(col(r.tkr, base)); });
                        row.col(|ui| {
                            ui.label(col(if r.side == Side::Bid { "B" } else { "S" }, base).strong());
                        });
                        row.col(|ui| { ui.label(col(r.typ, base)); });
                        row.col(|ui| { ui.label(col(format!("{:>6}", r.price), base)); });
                        row.col(|ui| { ui.label(col(format!("{:>5}", r.lots), base)); });
                        row.col(|ui| { ui.label(col(format!("{:>5}", r.filled), base)); });
                        row.col(|ui| { ui.label(col(r.stat, base)); });
                        row.col(|ui| {
                            if r.open && ui.add(Button::new(col("✕", RED)).small()).clicked() {
                                cancel = Some((r.stock, r.oid));
                            }
                        });
                    });
                });
        });
        if rows.is_empty() {
            ui.label(dim("── no orders ──"));
        }
        if let Some((stock, oid)) = cancel {
            self.app.cancel_player_order(stock, oid);
        }
    }

    fn trades_table(&mut self, ui: &mut egui::Ui) {
        section(ui, &format!("MY TRADES · {}", self.app.player.trades.len()));
        let rows: Vec<(String, &'static str, Side, String, String, String)> = self
            .app
            .player
            .trades
            .iter()
            .rev()
            .take(60)
            .map(|t| {
                (
                    mmss(t.ts),
                    self.app.defs[t.stock].ticker,
                    t.side,
                    idx::thousands(t.price),
                    idx::thousands(t.lots),
                    idx::thousands(t.fee),
                )
            })
            .collect();
        ui.push_id("mytrades", |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .column(Column::exact(44.0))
                .column(Column::exact(40.0))
                .column(Column::exact(16.0))
                .column(Column::exact(60.0))
                .column(Column::exact(52.0))
                .column(Column::remainder())
                .min_scrolled_height(60.0)
                .header(18.0, |mut h| {
                    for t in ["TIME", "TKR", "S", "PRICE", "LOTS", "FEE"] {
                        h.col(|ui| { ui.label(dim(t)); });
                    }
                })
                .body(|body| {
                    body.rows(18.0, rows.len(), |mut row| {
                        let (time, tkr, side, price, lots, fee) = &rows[row.index()];
                        let c = side_color(*side);
                        row.col(|ui| { ui.label(col(time.clone(), DIM)); });
                        row.col(|ui| { ui.label(fg(*tkr)); });
                        row.col(|ui| {
                            ui.label(col(if *side == Side::Bid { "B" } else { "S" }, c).strong());
                        });
                        row.col(|ui| { ui.label(col(format!("{price:>7}"), c)); });
                        row.col(|ui| { ui.label(fg(format!("{lots:>6}"))); });
                        row.col(|ui| { ui.label(col(format!("{fee:>8}"), DIM)); });
                    });
                });
        });
        if rows.is_empty() {
            ui.label(dim("── no trades yet ──"));
        }
    }

    // ---- center: stats + orderbook + queue ----

    fn center_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(BG).inner_margin(Margin::same(8)))
            .show(ctx, |ui| {
                let s = self.app.selected;
                let d = &self.app.defs[s];
                {
                    let m = &self.app.markets[s];
                    let chg = m.change();
                    let arrow = if chg > 0 { "▲" } else if chg < 0 { "▼" } else { "•" };
                    ui.horizontal(|ui| {
                        ui.label(col(d.ticker, WHITE).strong().size(17.0));
                        ui.label(dim(d.name));
                        if d.leverage > 1 {
                            ui.label(col(format!("⚡{}x", d.leverage), CYAN));
                        }
                        ui.label(
                            RichText::new(format!(
                                "{} {} {} ({:+.2}%)",
                                idx::thousands(m.last),
                                arrow,
                                idx::signed_thousands(chg),
                                m.change_pct()
                            ))
                            .color(chg_color(chg))
                            .strong()
                            .size(17.0),
                        );
                        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                            ui.label(dim("1m · session 09:00–09:10"));
                        });
                    });
                }
                let bottom_h = 232.0;
                let chart_h = (ui.available_height() - bottom_h).max(120.0);
                self.draw_chart(ui, s, chart_h);
                ui.add_space(4.0);

                // Portfolio metrics strip (Stockbit-style).
                let p = &self.app.player;
                let lasts = self.app.lasts();
                let stock_val: i64 = p
                    .positions
                    .iter()
                    .zip(lasts)
                    .map(|(pos, last)| pos.lots * SHARES_PER_LOT * last)
                    .sum();
                let debt: i64 = p.positions.iter().map(|x| x.debt).sum();
                let pnl = self.app.pnl();
                let pct = pnl as f64 * 100.0 / INITIAL_CASH as f64;
                let equity = self.app.equity();
                let metric = |ui: &mut egui::Ui, k: &str, v: String, c: Color32| {
                    ui.vertical(|ui| {
                        ui.label(dim(k));
                        ui.label(col(v, c).strong());
                    });
                    ui.add_space(14.0);
                };
                ui.horizontal(|ui| {
                    let p = &self.app.player;
                    metric(ui, "BALANCE", idx::thousands(p.cash), FG);
                    metric(ui, "LOCKED", idx::thousands(p.reserved_cash), FG);
                    metric(ui, "STOCK", idx::thousands(stock_val), FG);
                    metric(
                        ui,
                        "MARGIN DEBT",
                        idx::thousands(debt),
                        if debt > 0 { YELLOW } else { DIM },
                    );
                    metric(ui, "EQUITY", idx::thousands(equity), WHITE);
                    metric(
                        ui,
                        "NET P&L",
                        format!("{} ({:+.2}%)", idx::signed_thousands(pnl), pct),
                        chg_color(pnl),
                    );
                    metric(ui, "FEES", idx::thousands(p.fees), DIM);
                });
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    for (i, name) in ["PORTFOLIO", "ORDERS", "TRADE HISTORY"].iter().enumerate() {
                        let sel = self.center_tab == i;
                        let label = if sel {
                            col(*name, FOCUS).strong()
                        } else {
                            dim(*name)
                        };
                        if ui.selectable_label(sel, label).clicked() {
                            self.center_tab = i;
                        }
                    }
                });
                match self.center_tab {
                    0 => self.portfolio_table(ui),
                    1 => self.orders_table(ui),
                    _ => self.trades_table(ui),
                }
            });
    }

    /// Positions tab: one row per stock with mark-to-market and margin debt.
    fn portfolio_table(&mut self, ui: &mut egui::Ui) {
        let lasts = self.app.lasts();
        ui.push_id("portfolio", |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .column(Column::exact(52.0))
                .column(Column::exact(64.0))
                .column(Column::exact(72.0))
                .column(Column::exact(72.0))
                .column(Column::exact(100.0))
                .column(Column::exact(92.0))
                .column(Column::exact(100.0))
                .column(Column::remainder())
                .min_scrolled_height(80.0)
                .max_scroll_height(150.0)
                .header(18.0, |mut h| {
                    for t in ["TKR", "LOTS", "AVG", "LAST", "VALUE", "DEBT", "U-P/L", "REALIZED"] {
                        h.col(|ui| { ui.label(dim(t)); });
                    }
                })
                .body(|body| {
                    body.rows(20.0, self.app.defs.len(), |mut row| {
                        let i = row.index();
                        let d = &self.app.defs[i];
                        let pos = self.app.player.positions[i];
                        let m = &self.app.markets[i];
                        row.col(|ui| { ui.label(col(d.ticker, WHITE).strong()); });
                        if pos.lots == 0 {
                            row.col(|ui| { ui.label(dim("-")); });
                            row.col(|ui| { ui.label(dim("-")); });
                            row.col(|ui| {
                                ui.label(col(idx::thousands(lasts[i]), chg_color(m.change())));
                            });
                            row.col(|ui| { ui.label(dim("-")); });
                            row.col(|ui| { ui.label(dim("-")); });
                            row.col(|ui| { ui.label(dim("-")); });
                        } else {
                            let value = pos.lots * SHARES_PER_LOT * lasts[i];
                            let upl = ((lasts[i] as f64 - pos.avg)
                                * (pos.lots * SHARES_PER_LOT) as f64)
                                as i64;
                            row.col(|ui| { ui.label(fg(idx::thousands(pos.lots))); });
                            row.col(|ui| { ui.label(fg(idx::thousands(pos.avg.round() as i64))); });
                            row.col(|ui| {
                                ui.label(col(idx::thousands(lasts[i]), chg_color(m.change())));
                            });
                            row.col(|ui| { ui.label(fg(idx::thousands(value))); });
                            row.col(|ui| {
                                ui.label(if pos.debt > 0 {
                                    col(idx::thousands(pos.debt), YELLOW)
                                } else {
                                    dim("-")
                                });
                            });
                            row.col(|ui| {
                                ui.label(col(idx::signed_thousands(upl), chg_color(upl)));
                            });
                        }
                        row.col(|ui| {
                            ui.label(col(
                                idx::signed_thousands(pos.realized),
                                chg_color(pos.realized),
                            ));
                        });
                    });
                });
        });
    }

    /// 1-minute candlestick + volume chart drawn with the raw painter.
    fn draw_chart(&mut self, ui: &mut egui::Ui, s: usize, height: f32) {
        let m: &Market = &self.app.markets[s];
        let mut candles: Vec<Candle> = m.candles.clone();
        if self.app.mode != Mode::Ended {
            candles.push(m.forming_candle());
        }
        let (resp, painter) =
            ui.allocate_painter(Vec2::new(ui.available_width(), height), Sense::hover());
        painter.rect_filled(resp.rect, 3.0, PANEL);
        let rect = resp.rect.shrink(6.0);
        if candles.is_empty() {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "waiting for the first candle",
                FontId::proportional(12.0),
                DIM,
            );
            return;
        }

        let slots = (SESSION_SECS / 60) as usize + 1; // 10 closed minutes + the forming one
        let mut lo = m.prev_close.min(m.last);
        let mut hi = m.prev_close.max(m.last);
        for c in &candles {
            lo = lo.min(c.low);
            hi = hi.max(c.high);
        }
        let pad = (((hi - lo) as f64 * 0.08).ceil() as i64).max(idx::tick_size(hi));
        let (lo, hi) = (lo - pad, hi + pad);
        let span = (hi - lo).max(1) as f32;
        let price_h = rect.height() * 0.78;
        let vol_h = rect.height() * 0.16;
        let vol_top = rect.bottom() - vol_h;
        let y = |p: Price| rect.top() + (hi - p) as f32 / span * price_h;
        let slot_w = rect.width() / slots as f32;

        // Horizontal gridlines with price labels.
        for k in 0..=4i64 {
            let p = hi - (hi - lo) * k / 4;
            let yy = y(p);
            painter.line_segment(
                [egui::Pos2::new(rect.left(), yy), egui::Pos2::new(rect.right(), yy)],
                egui::Stroke::new(1.0, STRIPE),
            );
            painter.text(
                egui::Pos2::new(rect.right() - 2.0, yy - 1.0),
                Align2::RIGHT_BOTTOM,
                idx::thousands(p),
                FontId::monospace(9.0),
                DIM,
            );
        }
        // Previous close reference line.
        let py = y(m.prev_close);
        painter.line_segment(
            [egui::Pos2::new(rect.left(), py), egui::Pos2::new(rect.right(), py)],
            egui::Stroke::new(1.0, Color32::from_rgb(58, 64, 78)),
        );

        let max_vol = candles.iter().map(|c| c.vol).max().unwrap_or(1).max(1);
        for (i, c) in candles.iter().enumerate() {
            let cx = rect.left() + (i as f32 + 0.5) * slot_w;
            let bw = (slot_w * 0.55).clamp(3.0, 26.0);
            let color = match c.close.cmp(&c.open) {
                std::cmp::Ordering::Greater => GREEN,
                std::cmp::Ordering::Less => RED,
                std::cmp::Ordering::Equal => DIM,
            };
            painter.line_segment(
                [egui::Pos2::new(cx, y(c.high)), egui::Pos2::new(cx, y(c.low))],
                egui::Stroke::new(1.0, color),
            );
            let top = y(c.open.max(c.close));
            let bot = y(c.open.min(c.close)).max(top + 1.0);
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::Pos2::new(cx - bw / 2.0, top),
                    egui::Pos2::new(cx + bw / 2.0, bot),
                ),
                1.0,
                color,
            );
            // Volume bar.
            let vh = vol_h * c.vol as f32 / max_vol as f32;
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::Pos2::new(cx - bw / 2.0, vol_top + vol_h - vh),
                    egui::Pos2::new(cx + bw / 2.0, vol_top + vol_h),
                ),
                0.0,
                Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 96),
            );
            // Minute labels above the volume strip.
            if i % 2 == 1 {
                painter.text(
                    egui::Pos2::new(cx, vol_top - 2.0),
                    Align2::CENTER_BOTTOM,
                    format!("09:{:02}", i + 1),
                    FontId::monospace(9.0),
                    DIM,
                );
            }
        }
        // Last price marker.
        let ly = y(m.last);
        painter.line_segment(
            [egui::Pos2::new(rect.left(), ly), egui::Pos2::new(rect.right(), ly)],
            egui::Stroke::new(1.0, FOCUS),
        );
        painter.text(
            egui::Pos2::new(rect.left() + 2.0, ly - 2.0),
            Align2::LEFT_BOTTOM,
            idx::thousands(m.last),
            FontId::monospace(10.0),
            FOCUS,
        );
    }

    /// Floating sub-window showing the live order queue at a picked level.
    fn queue_window(&mut self, ctx: &egui::Context) {
        let Some((qs, side, price)) = self.queue_view else {
            return;
        };
        let ticker = self.app.defs[qs].ticker;
        let side_name = match side {
            Side::Bid => "BID",
            Side::Offer => "OFFER",
        };
        struct QRow {
            pos: usize,
            brk: &'static str,
            lots: String,
            age: String,
            kind: &'static str,
            color: Color32,
        }
        let show = self.revealed();
        let (rows, total, freq) = {
            let m = &self.app.markets[qs];
            match m.book.level_queue(side, price) {
                Some(q) => {
                    let total: i64 = q.iter().map(|o| o.remaining).sum();
                    let rows: Vec<QRow> = q
                        .iter()
                        .enumerate()
                        .take(300)
                        .map(|(i, o)| QRow {
                            pos: i + 1,
                            brk: if show || o.owner == OwnerId::Player {
                                o.broker
                            } else {
                                "··"
                            },
                            lots: idx::thousands(o.remaining),
                            age: format!(
                                "{}s",
                                self.app.tick.saturating_sub(o.ts) / TICKS_PER_SEC
                            ),
                            kind: match o.owner {
                                OwnerId::Player => "YOU",
                                _ if !show => "",
                                OwnerId::Agent(AgentKind::Lp) => "market maker",
                                OwnerId::Agent(AgentKind::Foreign) => "foreign",
                                OwnerId::Agent(AgentKind::Domestic) => "domestic",
                                OwnerId::Agent(AgentKind::Retail) => "retail",
                            },
                            color: if show || o.owner == OwnerId::Player {
                                owner_color(o.owner)
                            } else {
                                FG
                            },
                        })
                        .collect();
                    (rows, total, q.len())
                }
                None => (Vec::new(), 0, 0),
            }
        };
        let mut open = true;
        egui::Window::new(
            RichText::new(format!(
                "QUEUE · {ticker} · {side_name} @ {}",
                idx::thousands(price)
            ))
            .color(side_color(side))
            .strong(),
        )
        .id(egui::Id::new("queue_window"))
        .open(&mut open)
        .default_pos([430.0, 260.0])
        .default_width(360.0)
        .resizable(true)
        .show(ctx, |ui| {
            if rows.is_empty() {
                ui.label(dim("── no resting orders at this level ──"));
                ui.label(dim("(the level was fully consumed or cancelled)"));
                return;
            }
            ui.label(dim(format!(
                "{} lot · {} orders · price-time priority, top fills first",
                idx::thousands(total),
                freq
            )));
            ui.separator();
            ui.push_id("queue_win_table", |ui| {
                TableBuilder::new(ui)
                    .striped(true)
                    .column(Column::exact(30.0))
                    .column(Column::exact(40.0))
                    .column(Column::exact(90.0))
                    .column(Column::exact(50.0))
                    .column(Column::remainder())
                    .min_scrolled_height(60.0)
                    .max_scroll_height(340.0)
                    .header(18.0, |mut h| {
                        for t in ["#", "BRK", "LOTS", "AGE", "TYPE"] {
                            h.col(|ui| { ui.label(dim(t)); });
                        }
                    })
                    .body(|body| {
                        body.rows(18.0, rows.len(), |mut row| {
                            let r = &rows[row.index()];
                            let strong = r.kind == "YOU";
                            let style = |t: RichText| if strong { t.strong() } else { t };
                            row.col(|ui| { ui.label(dim(r.pos.to_string())); });
                            row.col(|ui| { ui.label(style(col(r.brk, r.color))); });
                            row.col(|ui| {
                                ui.label(style(col(format!("{:>8}", r.lots), r.color)));
                            });
                            row.col(|ui| { ui.label(dim(r.age.clone())); });
                            row.col(|ui| { ui.label(style(col(r.kind, r.color))); });
                        });
                    });
            });
        });
        if !open {
            self.queue_view = None;
        }
    }

    // ---- bottom: tapes ----


    fn summary_window(&mut self, ctx: &egui::Context) {
        if !self.post.summary {
            return;
        }
        let mut open = true;
        let equity = self.app.equity();
        let pnl = self.app.pnl();
        let pct = pnl as f64 * 100.0 / INITIAL_CASH as f64;
        let volume: i64 = self.app.player.trades.iter().map(|t| t.lots).sum();
        let realized: i64 = self.app.player.positions.iter().map(|p| p.realized).sum();
        let verdict = if pnl > 0 {
            ("PROFIT — mantap!", GREEN)
        } else if pnl < 0 {
            ("LOSS — the market always wins", RED)
        } else {
            ("FLAT — zero sum day", YELLOW)
        };
        egui::Window::new(RichText::new("SESSION COMPLETE — 10:00 MIN").color(FOCUS).strong())
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(verdict.0).color(verdict.1).strong().size(15.0));
                });
                ui.add_space(6.0);
                egui::Grid::new("summary").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                    let kv = |ui: &mut egui::Ui, k: &str, v: String, c: Color32| {
                        ui.label(dim(k));
                        ui.label(col(format!("{v:>24}"), c));
                        ui.end_row();
                    };
                    kv(ui, "Initial capital", idx::thousands(INITIAL_CASH), FG);
                    kv(ui, "Final equity", idx::thousands(equity), WHITE);
                    kv(
                        ui,
                        "Net P&L",
                        format!("{} ({:+.2}%)", idx::signed_thousands(pnl), pct),
                        chg_color(pnl),
                    );
                    kv(ui, "Realized (ex-fee)", idx::signed_thousands(realized), chg_color(realized));
                    kv(ui, "Fees paid", idx::thousands(self.app.player.fees), DIM);
                    kv(
                        ui,
                        "Trades / volume",
                        format!("{} / {} lot", self.app.player.trades.len(), idx::thousands(volume)),
                        FG,
                    );
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(Button::new(fg("Broker summary")).fill(BTN_DARK)).clicked() {
                        self.post.broker = true;
                    }
                    if ui.add(Button::new(fg("Done detail")).fill(BTN_DARK)).clicked() {
                        self.post.done = true;
                    }
                    if ui.add(Button::new(fg("Market summary")).fill(BTN_DARK)).clicked() {
                        self.post.market = true;
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add(Button::new(col("New session", FOCUS).strong()).fill(BTN_DARK)).clicked() {
                        self.app.restart();
                        self.next_tick = Instant::now() + tick_dur();
                        self.queue_view = None;
                        self.post = PostClose::default();
                    }
                    if ui.add(Button::new(fg("Quit")).fill(BTN_DARK)).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
        if !open {
            self.post.summary = false;
        }
    }

    /// Floating admin window: balance override and market-transparency toggle.
    fn admin_window(&mut self, ctx: &egui::Context) {
        if !self.admin_open {
            return;
        }
        let mut open = true;
        egui::Window::new(RichText::new("ADMIN").color(FOCUS).strong())
            .id(egui::Id::new("admin_window"))
            .open(&mut open)
            .default_pos([380.0, 120.0])
            .default_width(330.0)
            .resizable(false)
            .show(ctx, |ui| {
                section(ui, "BALANCE");
                egui::Grid::new("admin_cash").num_columns(2).spacing([16.0, 2.0]).show(ui, |ui| {
                    ui.label(dim("free cash"));
                    ui.label(fg(idx::thousands(self.app.player.cash)));
                    ui.end_row();
                    ui.label(dim("reserved (open bids)"));
                    ui.label(fg(idx::thousands(self.app.player.reserved_cash)));
                    ui.end_row();
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.admin_balance)
                            .desired_width(150.0)
                            .hint_text("new free cash (Rp)"),
                    );
                    let submit = ui.add(Button::new(fg("Set")).fill(BTN_DARK)).clicked()
                        || (resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
                    if submit {
                        let digits: String = self
                            .admin_balance
                            .chars()
                            .filter(|c| c.is_ascii_digit())
                            .collect();
                        match digits.parse::<i64>() {
                            Ok(v) => {
                                self.app.player.cash = v;
                                self.app.toast_ok(format!(
                                    "admin: free cash set to {}",
                                    idx::thousands(v)
                                ));
                            }
                            Err(_) => self.app.toast_err("admin: enter a valid amount"),
                        }
                    }
                });
                ui.label(dim("sets free cash; reserved cash on open bids is untouched"));
                ui.add_space(6.0);
                section(ui, "DISPLAY");
                ui.checkbox(&mut self.show_brokers, fg("Show broker codes & investor type"));
                ui.label(dim("off = real intraday tape: codes hidden until the close"));
            });
        if !open {
            self.admin_open = false;
        }
    }

    /// Post-close broker summary (per stock): buy/sell volume, value, average.
    fn broker_summary_window(&mut self, ctx: &egui::Context) {
        if !self.post.broker {
            return;
        }
        let mut open = true;
        egui::Window::new(RichText::new("BROKER SUMMARY · POST CLOSE").color(FOCUS).strong())
            .id(egui::Id::new("broker_summary_window"))
            .open(&mut open)
            .default_pos([40.0, 110.0])
            .default_width(620.0)
            .resizable(true)
            .show(ctx, |ui| {
                stock_tabs(ui, &self.app.defs, &mut self.post.broker_stock);
                let s = self.post.broker_stock;
                if self.post.cache.as_ref().map(|(cs, _)| *cs) != Some(s) {
                    self.post.cache = Some((s, broker_summary(&self.app.markets[s].log)));
                }
                let rows = &self.post.cache.as_ref().unwrap().1;
                ui.separator();
                if rows.is_empty() {
                    ui.label(dim("── no trades this session ──"));
                    return;
                }
                ui.push_id("broker_summary_table", |ui| {
                    TableBuilder::new(ui)
                        .striped(true)
                        .column(Column::exact(36.0))
                        .column(Column::exact(22.0))
                        .column(Column::exact(70.0))
                        .column(Column::exact(84.0))
                        .column(Column::exact(60.0))
                        .column(Column::exact(70.0))
                        .column(Column::exact(84.0))
                        .column(Column::exact(60.0))
                        .column(Column::remainder())
                        .min_scrolled_height(60.0)
                        .max_scroll_height(420.0)
                        .header(18.0, |mut h| {
                            for t in
                                ["BRK", "T", "B.VOL", "B.VAL", "B.AVG", "S.VOL", "S.VAL", "S.AVG", "NET LOT"]
                            {
                                h.col(|ui| { ui.label(dim(t)); });
                            }
                        })
                        .body(|body| {
                            body.rows(18.0, rows.len(), |mut row| {
                                let r = &rows[row.index()];
                                let strong = r.code == BROKER_PLAYER;
                                let style = |t: RichText| if strong { t.strong() } else { t };
                                row.col(|ui| { ui.label(style(col(r.code, broker_color(r.code)))); });
                                row.col(|ui| { ui.label(dim(investor_type(r.code))); });
                                row.col(|ui| { ui.label(style(col(idx::thousands(r.bvol), GREEN))); });
                                row.col(|ui| { ui.label(style(col(idx::compact(r.bval), GREEN))); });
                                row.col(|ui| { ui.label(dim(avg_px(r.bval, r.bvol))); });
                                row.col(|ui| { ui.label(style(col(idx::thousands(r.svol), RED))); });
                                row.col(|ui| { ui.label(style(col(idx::compact(r.sval), RED))); });
                                row.col(|ui| { ui.label(dim(avg_px(r.sval, r.svol))); });
                                let net = r.bvol - r.svol;
                                row.col(|ui| {
                                    ui.label(style(col(idx::signed_thousands(net), chg_color(net))));
                                });
                            });
                        });
                });
            });
        if !open {
            self.post.broker = false;
        }
    }

    /// Post-close done detail: the complete session tape with broker codes.
    fn done_detail_window(&mut self, ctx: &egui::Context) {
        if !self.post.done {
            return;
        }
        let mut open = true;
        egui::Window::new(RichText::new("DONE DETAIL · POST CLOSE").color(FOCUS).strong())
            .id(egui::Id::new("done_detail_window"))
            .open(&mut open)
            .default_pos([420.0, 110.0])
            .default_width(430.0)
            .resizable(true)
            .show(ctx, |ui| {
                stock_tabs(ui, &self.app.defs, &mut self.post.done_stock);
                let s = self.post.done_stock;
                let log = &self.app.markets[s].log;
                ui.separator();
                if log.is_empty() {
                    ui.label(dim("── no trades this session ──"));
                    return;
                }
                ui.label(dim(format!(
                    "{} trades · newest first",
                    idx::thousands(log.len() as i64)
                )));
                ui.push_id("done_detail_table", |ui| {
                    TableBuilder::new(ui)
                        .striped(true)
                        .column(Column::exact(64.0))
                        .column(Column::exact(84.0))
                        .column(Column::exact(70.0))
                        .column(Column::exact(56.0))
                        .column(Column::remainder())
                        .min_scrolled_height(60.0)
                        .max_scroll_height(440.0)
                        .header(18.0, |mut h| {
                            for t in ["TIME", "PRICE", "LOTS", "BUY", "SELL"] {
                                h.col(|ui| { ui.label(dim(t)); });
                            }
                        })
                        .body(|body| {
                            body.rows(18.0, log.len(), |mut row| {
                                let t = &log[log.len() - 1 - row.index()];
                                let (arrow, c) = match t.aggressor {
                                    Side::Bid => ("▲", GREEN),
                                    Side::Offer => ("▼", RED),
                                };
                                let you = t.buyer == OwnerId::Player
                                    || t.seller == OwnerId::Player;
                                let style = |txt: RichText| if you { txt.strong() } else { txt };
                                row.col(|ui| { ui.label(dim(self.app.clock_of_tick(t.ts))); });
                                row.col(|ui| {
                                    ui.label(style(col(
                                        format!("{:>7} {arrow}", idx::thousands(t.price)),
                                        c,
                                    )));
                                });
                                row.col(|ui| {
                                    ui.label(style(fg(format!("{:>8}", idx::thousands(t.lots)))));
                                });
                                row.col(|ui| {
                                    ui.label(style(col(t.buy_broker, broker_color(t.buy_broker))));
                                });
                                row.col(|ui| {
                                    ui.label(style(col(t.sell_broker, broker_color(t.sell_broker))));
                                });
                            });
                        });
                });
            });
        if !open {
            self.post.done = false;
        }
    }

    /// Post-close market summary: OHLC, change, volume, value, frequency per stock.
    fn market_summary_window(&mut self, ctx: &egui::Context) {
        if !self.post.market {
            return;
        }
        let mut open = true;
        egui::Window::new(RichText::new("MARKET SUMMARY · POST CLOSE").color(FOCUS).strong())
            .id(egui::Id::new("market_summary_window"))
            .open(&mut open)
            .default_pos([120.0, 320.0])
            .default_width(780.0)
            .resizable(true)
            .show(ctx, |ui| {
                let (mut tvol, mut tval, mut tfreq) = (0i64, 0i64, 0u64);
                let (mut up, mut down, mut flat) = (0u32, 0u32, 0u32);
                for m in &self.app.markets {
                    tvol += m.volume_lots;
                    tval += m.value;
                    tfreq += m.trade_count;
                    match m.change().cmp(&0) {
                        std::cmp::Ordering::Greater => up += 1,
                        std::cmp::Ordering::Less => down += 1,
                        std::cmp::Ordering::Equal => flat += 1,
                    }
                }
                ui.push_id("market_summary_table", |ui| {
                    TableBuilder::new(ui)
                        .striped(true)
                        .column(Column::exact(52.0))
                        .column(Column::exact(62.0))
                        .column(Column::exact(62.0))
                        .column(Column::exact(62.0))
                        .column(Column::exact(62.0))
                        .column(Column::exact(62.0))
                        .column(Column::exact(110.0))
                        .column(Column::exact(80.0))
                        .column(Column::exact(72.0))
                        .column(Column::remainder())
                        .header(18.0, |mut h| {
                            for t in
                                ["STOCK", "PREV", "OPEN", "HIGH", "LOW", "CLOSE", "+/-", "VOL", "VALUE", "FREQ"]
                            {
                                h.col(|ui| { ui.label(dim(t)); });
                            }
                        })
                        .body(|body| {
                            body.rows(18.0, self.app.defs.len(), |mut row| {
                                let i = row.index();
                                let d = &self.app.defs[i];
                                let m = &self.app.markets[i];
                                let chg = m.change();
                                row.col(|ui| { ui.label(fg(d.ticker).strong()); });
                                row.col(|ui| { ui.label(dim(idx::thousands(m.prev_close))); });
                                row.col(|ui| { ui.label(fg(idx::thousands(m.open))); });
                                row.col(|ui| { ui.label(col(idx::thousands(m.high), GREEN)); });
                                row.col(|ui| { ui.label(col(idx::thousands(m.low), RED)); });
                                row.col(|ui| { ui.label(col(idx::thousands(m.last), WHITE).strong()); });
                                row.col(|ui| {
                                    ui.label(col(
                                        format!(
                                            "{} ({:+.2}%)",
                                            idx::signed_thousands(chg),
                                            m.change_pct()
                                        ),
                                        chg_color(chg),
                                    ));
                                });
                                row.col(|ui| { ui.label(fg(idx::thousands(m.volume_lots))); });
                                row.col(|ui| { ui.label(fg(idx::compact(m.value))); });
                                row.col(|ui| { ui.label(fg(idx::thousands(m.trade_count as i64))); });
                            });
                        });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(dim("total"));
                    ui.label(fg(format!("{} lot", idx::thousands(tvol))));
                    ui.label(dim("·"));
                    ui.label(fg(idx::compact(tval)));
                    ui.label(dim("·"));
                    ui.label(fg(format!("{} trades", idx::thousands(tfreq as i64))));
                    ui.label(dim("▏"));
                    ui.label(col(format!("▲ {up}"), GREEN));
                    ui.label(col(format!("▼ {down}"), RED));
                    ui.label(dim(format!("― {flat}")));
                });
            });
        if !open {
            self.post.market = false;
        }
    }
}

struct TapeRow {
    time: String,
    tkr: &'static str,
    price: String,
    arrow_color: Color32,
    lots: String,
    brokers: String,
    player: bool,
}

fn tape_rows<'a>(
    app: &App,
    it: impl Iterator<Item = &'a Trade>,
    show_brokers: bool,
) -> Vec<TapeRow> {
    it.map(|t| {
        let (arrow, c) = match t.aggressor {
            Side::Bid => ("▲", GREEN),
            Side::Offer => ("▼", RED),
        };
        let buyer_you = t.buyer == OwnerId::Player;
        let seller_you = t.seller == OwnerId::Player;
        let brokers = if show_brokers {
            format!("{}▸{}", t.buy_broker, t.sell_broker)
        } else {
            // Real intraday tape: counterparty codes hidden, only own fills marked.
            match (buyer_you, seller_you) {
                (true, true) => "YOU▸YOU".to_string(),
                (true, false) => "YOU▸".to_string(),
                (false, true) => "▸YOU".to_string(),
                (false, false) => String::new(),
            }
        };
        TapeRow {
            time: app.clock_of_tick(t.ts),
            tkr: app.defs[t.stock].ticker,
            price: format!("{:>7} {arrow}", idx::thousands(t.price)),
            arrow_color: c,
            lots: format!("{:>8}", idx::thousands(t.lots)),
            brokers,
            player: buyer_you || seller_you,
        }
    })
    .collect()
}

fn draw_tape(ui: &mut egui::Ui, title: &str, id: &str, rows: &[TapeRow], show_ticker: bool) {
    section(ui, title);
    if rows.is_empty() {
        ui.label(dim("── waiting for prints ──"));
        return;
    }
    ui.push_id(id, |ui| {
        let mut tb = TableBuilder::new(ui)
            .striped(true)
            .column(Column::exact(66.0));
        if show_ticker {
            tb = tb.column(Column::exact(44.0));
        }
        tb.column(Column::exact(80.0))
            .column(Column::exact(70.0))
            .column(Column::remainder())
            .min_scrolled_height(60.0)
            .body(|body| {
                body.rows(18.0, rows.len(), |mut row| {
                    let r = &rows[row.index()];
                    row.col(|ui| { ui.label(col(r.time.clone(), DIM)); });
                    if show_ticker {
                        row.col(|ui| { ui.label(fg(r.tkr)); });
                    }
                    row.col(|ui| { ui.label(col(r.price.clone(), r.arrow_color).strong()); });
                    row.col(|ui| { ui.label(fg(r.lots.clone())); });
                    row.col(|ui| {
                        let c = if r.player { CYAN } else { DIM };
                        let t = col(r.brokers.clone(), c);
                        ui.label(if r.player { t.strong() } else { t });
                    });
                });
            });
    });
}

/// Row of ticker tabs used by the post-close windows.
fn stock_tabs(ui: &mut egui::Ui, defs: &[StockDef], sel: &mut usize) {
    ui.horizontal(|ui| {
        for (i, d) in defs.iter().enumerate() {
            let txt = if *sel == i {
                col(d.ticker, FOCUS).strong()
            } else {
                dim(d.ticker)
            };
            if ui.selectable_label(*sel == i, txt).clicked() {
                *sel = i;
            }
        }
    });
}

/// Per-broker aggregates over one stock's full-session trade log.
struct BrokerRow {
    code: &'static str,
    bvol: Lots,
    bval: i64,
    svol: Lots,
    sval: i64,
}

fn upsert<'a>(rows: &'a mut Vec<BrokerRow>, code: &'static str) -> &'a mut BrokerRow {
    if let Some(i) = rows.iter().position(|r| r.code == code) {
        &mut rows[i]
    } else {
        rows.push(BrokerRow { code, bvol: 0, bval: 0, svol: 0, sval: 0 });
        rows.last_mut().unwrap()
    }
}

/// IDX-style broker summary: both sides of every print, sorted by turnover.
fn broker_summary(log: &[Trade]) -> Vec<BrokerRow> {
    let mut rows: Vec<BrokerRow> = Vec::new();
    for t in log {
        let val = t.lots * SHARES_PER_LOT * t.price;
        let b = upsert(&mut rows, t.buy_broker);
        b.bvol += t.lots;
        b.bval += val;
        let s = upsert(&mut rows, t.sell_broker);
        s.svol += t.lots;
        s.sval += val;
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.bval + r.sval));
    rows
}

/// Average price per share, "—" when nothing traded.
fn avg_px(val: i64, lots: Lots) -> String {
    if lots <= 0 {
        "—".into()
    } else {
        idx::thousands(val / (lots * SHARES_PER_LOT))
    }
}

/// IDX price coloring vs previous close: up green, down red, unchanged yellow.
fn px_color(prev: Price, p: Price) -> Color32 {
    match p.cmp(&prev) {
        std::cmp::Ordering::Greater => GREEN,
        std::cmp::Ordering::Less => RED,
        std::cmp::Ordering::Equal => YELLOW,
    }
}
