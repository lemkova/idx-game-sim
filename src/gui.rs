//! Native GUI frontend (eframe/egui) — professional dark trading-terminal look.

use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Button, Color32, CursorIcon, DragValue, FontId, Key, Label, Margin,
    RichText, Sense, Vec2,
};
use egui_extras::{Column, TableBuilder};

use crate::app::{App, Mode, BOOK_DEPTH, TICKS_PER_SEC};
use crate::idx;
use crate::player::{OrderStatus, INITIAL_CASH};
use crate::types::{AgentKind, OrdType, OwnerId, Price, Side, SHARES_PER_LOT};

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
    queue_view: Option<(usize, Side, Price)>, // (stock, side, price) under inspection
}

impl GuiApp {
    fn new(seed: u64) -> Self {
        let mut app = App::new(seed);
        app.open_form(Side::Bid, None); // sensible initial ticket price
        GuiApp {
            app,
            next_tick: Instant::now() + tick_dur(),
            ticket_stock: 0,
            queue_view: None,
        }
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

        self.header(ctx);
        self.status_bar(ctx);
        self.tapes(ctx);
        self.left_panel(ctx);
        self.right_panel(ctx);
        self.center_panel(ctx);
        self.queue_window(ctx);

        if self.app.mode == Mode::Ended {
            self.summary_window(ctx);
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
            }
            if i.key_pressed(Key::S) {
                self.app.open_form(Side::Offer, None);
            }
            if i.key_pressed(Key::P) && self.app.mode != Mode::Ended {
                self.app.toggle_pause();
            }
            if i.key_pressed(Key::N) && self.app.mode == Mode::Ended {
                self.app.restart();
                self.next_tick = Instant::now() + tick_dur();
                self.queue_view = None;
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

                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(Button::new(fg("Quit")).fill(BTN_DARK)).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui.add(Button::new(fg("New session")).fill(BTN_DARK)).clicked() {
                            app.restart();
                            self.next_tick = Instant::now() + tick_dur();
                            self.queue_view = None;
                        }
                        if app.mode != Mode::Ended {
                            let label = if app.mode == Mode::Paused { "Resume" } else { "Pause" };
                            if ui.add(Button::new(fg(label)).fill(BTN_DARK)).clicked() {
                                app.toggle_pause();
                            }
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
                let rows: Vec<(String, String, i64, String, String)> = self
                    .app
                    .defs
                    .iter()
                    .zip(self.app.markets.iter())
                    .map(|(d, m)| {
                        (
                            d.ticker.to_string(),
                            idx::thousands(m.last),
                            m.change(),
                            format!("{:+.2}%", m.change_pct()),
                            idx::compact(m.volume_lots),
                        )
                    })
                    .collect();
                let selected = self.app.selected;
                let mut clicked: Option<usize> = None;
                ui.push_id("watchlist", |ui| {
                    TableBuilder::new(ui)
                        .striped(true)
                        .sense(Sense::click())
                        .column(Column::exact(56.0))
                        .column(Column::exact(72.0))
                        .column(Column::exact(72.0))
                        .column(Column::remainder())
                        .header(18.0, |mut h| {
                            h.col(|ui| { ui.label(dim("TKR")); });
                            h.col(|ui| { ui.label(dim("LAST")); });
                            h.col(|ui| { ui.label(dim("CHG%")); });
                            h.col(|ui| { ui.label(dim("VOL")); });
                        })
                        .body(|body| {
                            body.rows(20.0, rows.len(), |mut row| {
                                let i = row.index();
                                row.set_selected(i == selected);
                                let (tkr, last, chg, pct, vol) = &rows[i];
                                let c = chg_color(*chg);
                                row.col(|ui| { ui.label(col(tkr.clone(), WHITE).strong()); });
                                row.col(|ui| { ui.label(col(format!("{last:>7}"), c)); });
                                row.col(|ui| { ui.label(col(format!("{pct:>7}"), c)); });
                                row.col(|ui| { ui.label(col(format!("{vol:>7}"), FG)); });
                                if row.response().clicked() {
                                    clicked = Some(i);
                                }
                            });
                        });
                });
                if let Some(i) = clicked {
                    self.app.select_stock(i);
                }

                section(ui, "PORTFOLIO");
                let lasts = self.app.lasts();
                egui::Grid::new("positions")
                    .num_columns(4)
                    .spacing([10.0, 3.0])
                    .show(ui, |ui| {
                        ui.label(dim("TKR"));
                        ui.label(dim("LOTS"));
                        ui.label(dim("AVG"));
                        ui.label(dim("U-P/L"));
                        ui.end_row();
                        for (i, d) in self.app.defs.iter().enumerate() {
                            let pos = self.app.player.positions[i];
                            ui.label(col(d.ticker, WHITE));
                            if pos.lots == 0 {
                                ui.label(dim("-"));
                                ui.label(dim("-"));
                                ui.label(dim("-"));
                            } else {
                                let upl = ((lasts[i] as f64 - pos.avg)
                                    * (pos.lots * SHARES_PER_LOT) as f64)
                                    as i64;
                                ui.label(fg(idx::thousands(pos.lots)));
                                ui.label(fg(idx::thousands(pos.avg.round() as i64)));
                                ui.label(col(idx::signed_thousands(upl), chg_color(upl)));
                            }
                            ui.end_row();
                        }
                    });

                ui.separator();
                let p = &self.app.player;
                let stock_val: i64 = p
                    .positions
                    .iter()
                    .zip(lasts)
                    .map(|(pos, last)| pos.lots * SHARES_PER_LOT * last)
                    .sum();
                let realized: i64 = p.positions.iter().map(|x| x.realized).sum();
                let pnl = self.app.pnl();
                let pct = pnl as f64 * 100.0 / INITIAL_CASH as f64;
                egui::Grid::new("totals")
                    .num_columns(2)
                    .spacing([10.0, 3.0])
                    .show(ui, |ui| {
                        let kv = |ui: &mut egui::Ui, k: &str, v: String, c: Color32| {
                            ui.label(dim(k));
                            ui.label(col(format!("{v:>16}"), c));
                            ui.end_row();
                        };
                        kv(ui, "CASH", idx::thousands(p.cash), FG);
                        kv(ui, "LOCKED", idx::thousands(p.reserved_cash), FG);
                        kv(ui, "STOCK", idx::thousands(stock_val), FG);
                        kv(ui, "EQUITY", idx::thousands(self.app.equity()), WHITE);
                        kv(ui, "REALIZED", idx::signed_thousands(realized), chg_color(realized));
                        kv(ui, "FEES", idx::thousands(p.fees), DIM);
                        kv(
                            ui,
                            "NET P&L",
                            format!("{} {:+.2}%", idx::signed_thousands(pnl), pct),
                            chg_color(pnl),
                        );
                    });
            });
    }

    // ---- right: order ticket + orders + trades ----

    fn right_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("right")
            .exact_width(380.0)
            .resizable(false)
            .show(ctx, |ui| {
                self.ticket(ui);
                self.orders_table(ui);
                self.trades_table(ui);
            });
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
        section(ui, "ORDER TICKET");
        ui.horizontal(|ui| {
            ui.label(col(ticker, WHITE).strong().size(14.0));
            ui.label(dim(name));
        });

        let enabled = self.app.mode != Mode::Ended;
        ui.add_enabled_ui(enabled, |ui| {
            // side
            ui.horizontal(|ui| {
                let side = self.app.entry.side;
                let buy_fill = if side == Side::Bid { GREEN_DARK } else { BTN_DARK };
                let sell_fill = if side == Side::Offer { RED_DARK } else { BTN_DARK };
                let w = (ui.available_width() - 8.0) / 2.0;
                if ui
                    .add_sized([w, 26.0], Button::new(col("BUY", GREEN).strong()).fill(buy_fill))
                    .clicked()
                {
                    self.app.open_form(Side::Bid, None);
                }
                if ui
                    .add_sized([w, 26.0], Button::new(col("SELL", RED).strong()).fill(sell_fill))
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
            });
            // submit
            let side = self.app.entry.side;
            let (label, fill) = match side {
                Side::Bid => (format!("SUBMIT BUY {ticker}"), GREEN_DARK),
                Side::Offer => (format!("SUBMIT SELL {ticker}"), RED_DARK),
            };
            if ui
                .add_sized(
                    [ui.available_width(), 30.0],
                    Button::new(col(label, side_color(side)).strong()).fill(fill),
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
                let (ticker, name) = (self.app.defs[s].ticker, self.app.defs[s].name);

                // stats block
                {
                    let m = &self.app.markets[s];
                    let chg = m.change();
                    let arrow = if chg > 0 { "▲" } else if chg < 0 { "▼" } else { "•" };
                    ui.horizontal(|ui| {
                        ui.label(col(ticker, WHITE).strong().size(17.0));
                        ui.label(dim(name));
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
                    });
                    ui.horizontal(|ui| {
                        ui.label(dim("O"));
                        ui.label(fg(idx::thousands(m.open)));
                        ui.label(dim("H"));
                        ui.label(fg(idx::thousands(m.high)));
                        ui.label(dim("L"));
                        ui.label(fg(idx::thousands(m.low)));
                        ui.label(dim("· VOL"));
                        ui.label(fg(format!("{} lot", idx::compact(m.volume_lots))));
                        ui.label(dim("VAL"));
                        ui.label(fg(idx::compact(m.value)));
                        ui.label(dim("FRQ"));
                        ui.label(fg(idx::thousands(m.trade_count as i64)));
                        ui.label(dim("· ARA"));
                        ui.label(col(idx::thousands(m.upper_bound), GREEN));
                        ui.label(dim("ARB"));
                        ui.label(col(idx::thousands(m.lower_bound), RED));
                    });
                }
                ui.separator();

                // book halves
                let bids = self.app.markets[s].levels(Side::Bid, BOOK_DEPTH);
                let offers = self.app.markets[s].levels(Side::Offer, BOOK_DEPTH);
                let mark = |side: Side, price: Price| -> bool {
                    self.app.player.open_orders().any(|o| {
                        o.stock == s && o.side == side && o.otype == OrdType::Limit && o.price == price
                    })
                };
                let bid_rows: Vec<(String, String, String, Price)> = bids
                    .iter()
                    .map(|l| {
                        (
                            format!("{:>4}", l.freq),
                            format!("{:>9}", idx::thousands(l.lots)),
                            format!(
                                "{}{:>7}",
                                if mark(Side::Bid, l.price) { "•" } else { " " },
                                idx::thousands(l.price)
                            ),
                            l.price,
                        )
                    })
                    .collect();
                let offer_rows: Vec<(String, String, String, Price)> = offers
                    .iter()
                    .map(|l| {
                        (
                            format!(
                                "{:<7}{}",
                                idx::thousands(l.price),
                                if mark(Side::Offer, l.price) { "•" } else { " " }
                            ),
                            format!("{:>9}", idx::thousands(l.lots)),
                            format!("{:>4}", l.freq),
                            l.price,
                        )
                    })
                    .collect();

                // Row click (price area) only selects the level for the ticket;
                // clicking the LOTS number opens the order-queue window.
                let mut price_pick: Option<Price> = None;
                let mut queue_pick: Option<(Side, Price)> = None;
                let sel_bid: Option<Price> = match self.queue_view {
                    Some((qs, Side::Bid, qp)) if qs == s => Some(qp),
                    _ => None,
                };
                let sel_offer: Option<Price> = match self.queue_view {
                    Some((qs, Side::Offer, qp)) if qs == s => Some(qp),
                    _ => None,
                };
                ui.columns(2, |cols| {
                    cols[0].push_id("bidbook", |ui| {
                        TableBuilder::new(ui)
                            .striped(true)
                            .sense(Sense::click())
                            .column(Column::exact(48.0))
                            .column(Column::exact(96.0))
                            .column(Column::remainder())
                            .header(18.0, |mut h| {
                                h.col(|ui| { ui.label(dim("FREQ")); });
                                h.col(|ui| { ui.label(dim("LOTS")); });
                                h.col(|ui| { ui.label(col("BID", GREEN).strong()); });
                            })
                            .body(|body| {
                                body.rows(19.0, BOOK_DEPTH, |mut row| {
                                    let r = row.index();
                                    row.set_selected(
                                        sel_bid.is_some()
                                            && bid_rows.get(r).map(|x| x.3) == sel_bid,
                                    );
                                    if let Some((freq, lots, price, px)) = bid_rows.get(r) {
                                        row.col(|ui| { ui.label(col(freq.clone(), DIM)); });
                                        row.col(|ui| {
                                            let resp = ui
                                                .add(Label::new(fg(lots.clone())).sense(Sense::click()))
                                                .on_hover_cursor(CursorIcon::PointingHand)
                                                .on_hover_text("view order queue");
                                            if resp.clicked() {
                                                queue_pick = Some((Side::Bid, *px));
                                            }
                                        });
                                        row.col(|ui| { ui.label(col(price.clone(), GREEN).strong()); });
                                        if row.response().clicked() {
                                            price_pick = Some(*px);
                                        }
                                    } else {
                                        row.col(|_| {});
                                        row.col(|_| {});
                                        row.col(|_| {});
                                    }
                                });
                            });
                    });
                    cols[1].push_id("offerbook", |ui| {
                        TableBuilder::new(ui)
                            .striped(true)
                            .sense(Sense::click())
                            .column(Column::remainder())
                            .column(Column::exact(96.0))
                            .column(Column::exact(48.0))
                            .header(18.0, |mut h| {
                                h.col(|ui| { ui.label(col("OFFER", RED).strong()); });
                                h.col(|ui| { ui.label(dim("LOTS")); });
                                h.col(|ui| { ui.label(dim("FREQ")); });
                            })
                            .body(|body| {
                                body.rows(19.0, BOOK_DEPTH, |mut row| {
                                    let r = row.index();
                                    row.set_selected(
                                        sel_offer.is_some()
                                            && offer_rows.get(r).map(|x| x.3) == sel_offer,
                                    );
                                    if let Some((price, lots, freq, px)) = offer_rows.get(r) {
                                        row.col(|ui| { ui.label(col(price.clone(), RED).strong()); });
                                        row.col(|ui| {
                                            let resp = ui
                                                .add(Label::new(fg(lots.clone())).sense(Sense::click()))
                                                .on_hover_cursor(CursorIcon::PointingHand)
                                                .on_hover_text("view order queue");
                                            if resp.clicked() {
                                                queue_pick = Some((Side::Offer, *px));
                                            }
                                        });
                                        row.col(|ui| { ui.label(col(freq.clone(), DIM)); });
                                        if row.response().clicked() {
                                            price_pick = Some(*px);
                                        }
                                    } else {
                                        row.col(|_| {});
                                        row.col(|_| {});
                                        row.col(|_| {});
                                    }
                                });
                            });
                    });
                });
                if let Some(price) = price_pick {
                    self.app.entry.price = price;
                }
                if let Some((side, price)) = queue_pick {
                    self.queue_view = Some((s, side, price));
                    self.app.entry.price = price;
                }
                ui.add_space(2.0);
                ui.label(dim(
                    "click a price → set ticket price · click a LOTS number → open its order queue",
                ));
            });
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
                            brk: o.broker,
                            lots: idx::thousands(o.remaining),
                            age: format!(
                                "{}s",
                                self.app.tick.saturating_sub(o.ts) / TICKS_PER_SEC
                            ),
                            kind: match o.owner {
                                OwnerId::Player => "YOU",
                                OwnerId::Agent(AgentKind::Lp) => "market maker",
                                OwnerId::Agent(AgentKind::Foreign) => "foreign",
                                OwnerId::Agent(AgentKind::Domestic) => "domestic",
                                OwnerId::Agent(AgentKind::Retail) => "retail",
                            },
                            color: owner_color(o.owner),
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

    fn tapes(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("tapes")
            .exact_height(200.0)
            .frame(egui::Frame::default().fill(PANEL).inner_margin(Margin::symmetric(8, 4)))
            .show(ctx, |ui| {
                let s = self.app.selected;
                let stock_rows = tape_rows(
                    &self.app,
                    self.app.markets[s].tape.iter().rev().take(48),
                    false,
                );
                let global_rows =
                    tape_rows(&self.app, self.app.global_tape.iter().rev().take(48), true);
                let stock_title = format!("TAPE · {}", self.app.defs[s].ticker);
                ui.columns(2, |cols| {
                    draw_tape(&mut cols[0], &stock_title, "tape_stock", &stock_rows, false);
                    draw_tape(&mut cols[1], "TAPE · ALL", "tape_all", &global_rows, true);
                });
            });
    }

    fn summary_window(&mut self, ctx: &egui::Context) {
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
                    if ui.add(Button::new(col("New session", FOCUS).strong()).fill(BTN_DARK)).clicked() {
                        self.app.restart();
                        self.next_tick = Instant::now() + tick_dur();
                        self.queue_view = None;
                    }
                    if ui.add(Button::new(fg("Quit")).fill(BTN_DARK)).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
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
    it: impl Iterator<Item = &'a crate::types::Trade>,
    _global: bool,
) -> Vec<TapeRow> {
    it.map(|t| {
        let (arrow, c) = match t.aggressor {
            Side::Bid => ("▲", GREEN),
            Side::Offer => ("▼", RED),
        };
        TapeRow {
            time: app.clock_of_tick(t.ts),
            tkr: app.defs[t.stock].ticker,
            price: format!("{:>7} {arrow}", idx::thousands(t.price)),
            arrow_color: c,
            lots: format!("{:>8}", idx::thousands(t.lots)),
            brokers: format!("{}▸{}", t.buy_broker, t.sell_broker),
            player: t.buyer == OwnerId::Player || t.seller == OwnerId::Player,
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
