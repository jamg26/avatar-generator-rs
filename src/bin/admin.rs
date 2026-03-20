//! AvaGen Admin — native desktop GUI (egui/eframe).
//!
//! Build:  cargo build --release --bin avagen-admin
//! Run:    ./target/release/avagen-admin
//!         (reads DATABASE_URL / ADMIN_SECRET / API_BASE_URL from .env)

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use dotenvy::dotenv;
use eframe::egui::{
    self, Color32, Frame, Margin, Rounding, RichText, ScrollArea, Stroke,
    TextEdit, Ui, Vec2,
};
use egui_extras::{Column, TableBuilder};
use egui_plot::{Bar, BarChart, Plot};
use reqwest::Client;
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, PgPool};
use tokio::runtime::Runtime;
use uuid::Uuid;

// ─── Design tokens ────────────────────────────────────────────────────────────

fn c_accent()     -> Color32 { Color32::from_rgb(99, 102, 241) }
fn c_accent_dim() -> Color32 { Color32::from_rgb(67, 56, 202) }
fn c_success()    -> Color32 { Color32::from_rgb(34, 197, 94) }
fn c_warning()    -> Color32 { Color32::from_rgb(234, 179, 8) }
fn c_danger()     -> Color32 { Color32::from_rgb(239, 68, 68) }
fn c_danger_dim() -> Color32 { Color32::from_rgb(185, 28, 28) }
fn c_text()       -> Color32 { Color32::from_rgb(241, 245, 249) }
fn c_text_dim()   -> Color32 { Color32::from_rgb(148, 163, 184) }
fn c_text_faint() -> Color32 { Color32::from_rgb(71, 85, 105) }
fn c_bg_deep()    -> Color32 { Color32::from_rgb(8, 12, 20) }
fn c_bg_side()    -> Color32 { Color32::from_rgb(12, 18, 30) }
fn c_bg_panel()   -> Color32 { Color32::from_rgb(15, 23, 42) }
fn c_bg_card()    -> Color32 { Color32::from_rgb(22, 33, 56) }
fn c_bg_input()   -> Color32 { Color32::from_rgb(30, 42, 70) }
fn c_border()     -> Color32 { Color32::from_rgb(44, 60, 96) }
fn c_border_hi()  -> Color32 { Color32::from_rgb(70, 88, 128) }

fn card_frame() -> Frame {
    Frame::none()
        .fill(c_bg_card())
        .stroke(Stroke::new(1.0, c_border()))
        .rounding(Rounding::same(8.0))
        .inner_margin(Margin::same(16.0))
}

fn subtle_frame() -> Frame {
    Frame::none()
        .fill(c_bg_panel())
        .stroke(Stroke::new(1.0, c_border()))
        .rounding(Rounding::same(6.0))
        .inner_margin(Margin::same(12.0))
}

// ─── DB row types ─────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow, Clone)]
struct DbKey {
    id:            String,
    name:          String,
    key_prefix:    String,
    monthly_quota: i64,
    is_active:     bool,
    created_at:    DateTime<Utc>,
    updated_at:    DateTime<Utc>,
    monthly_used:  i64,
    total_used:    i64,
}

#[derive(sqlx::FromRow, Clone)]
struct DbRecent {
    name:       Option<String>,
    key_prefix: Option<String>,
    endpoint:   String,
    count:      i64,
    created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow, Clone)]
struct DbJob {
    id:             Uuid,
    state:          String,
    total:          i64,
    completed:      i64,
    failed_count:   i64,
    model:          String,
    key_name:       Option<String>,
    queue_position: Option<i64>,
    created_at:     DateTime<Utc>,
}

#[derive(sqlx::FromRow)] struct DbDaily  { d: chrono::NaiveDate, n: i64 }
#[derive(sqlx::FromRow)] struct DbHourly { h: i32, n: i64 }
#[derive(sqlx::FromRow)] struct DbPerKey { name: String, monthly_quota: i64, used: i64 }
#[derive(sqlx::FromRow)] struct DbEndpt  { endpoint: String, n: i64 }

// ─── UI-ready data ────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
struct DashData {
    total_keys:  i64,
    active_keys: i64,
    today_req:   i64,
    month_req:   i64,
    active_jobs: i64,
    health:      String,
    recent:      Vec<UiRecent>,
}

#[derive(Clone)]
struct UiRecent { name: String, prefix: String, endpoint: String, count: i64, when: String }

#[derive(Clone)]
struct UiKey {
    id:            String,
    name:          String,
    prefix:        String,
    monthly_quota: i64,
    is_active:     bool,
    monthly_used:  i64,
    total_used:    i64,
    created_at:    String,
}

#[derive(Clone, Default)]
struct MetricsData {
    daily:     Vec<(String, f64)>,
    hourly:    Vec<(i32, f64)>,
    per_key:   Vec<(String, i64, i64)>,
    endpoints: Vec<(String, i64)>,
}

#[derive(Clone)]
struct UiJob {
    id:      Uuid,
    id_str:  String,
    state:   String,
    total:   i64,
    done:    i64,
    failed:  i64,
    model:   String,
    key:     String,
    qpos:    String,
    created: String,
    active:  bool,
}

#[derive(Clone, Default)]
struct SysData {
    api_ok: bool, api_err: String, api_url: String,
    db_ok: bool,  db_ver: String,
    db_keys: i64, db_logs: i64, db_jobs: i64,
}

// ─── Shared async state ───────────────────────────────────────────────────────

#[derive(Default)]
struct Shared {
    dash: Option<DashData>,    dash_load: bool,
    keys: Vec<UiKey>,          keys_load: bool,  keys_err: Option<String>,
    metrics: Option<MetricsData>, met_load: bool,
    jobs: Vec<UiJob>,          jobs_load: bool,
    sys: Option<SysData>,      sys_load: bool,
}

// ─── Confirmation dialog ──────────────────────────────────────────────────────

#[derive(Clone)]
enum Confirm {
    CancelJob { id: Uuid,   label: String },
    DeleteJob { id: Uuid,   label: String },
    RevokeKey { id: String, name: String },
    DeleteKey { id: String, name: String },
}

impl Confirm {
    fn title(&self) -> &str {
        match self {
            Self::CancelJob { .. } => "Cancel Job",
            Self::DeleteJob { .. } => "Delete Job",
            Self::RevokeKey { .. } => "Revoke API Key",
            Self::DeleteKey { .. } => "Delete API Key",
        }
    }
    fn message(&self) -> String {
        match self {
            Self::CancelJob { label, .. } => format!("Cancel job {}?\n\nThis will stop generation early.", label),
            Self::DeleteJob { label, .. } => format!("Delete job record {}?\n\nThis is permanent.", label),
            Self::RevokeKey { name, .. }  => format!("Revoke key \"{name}\"?\n\nClients using it will receive 401 errors."),
            Self::DeleteKey { name, .. }  => format!("Permanently delete key \"{name}\" and all its usage logs?\n\nThis cannot be undone."),
        }
    }
    fn confirm_label(&self) -> &str {
        match self {
            Self::CancelJob { .. } => "Cancel Job",
            Self::DeleteJob { .. } => "Delete",
            Self::RevokeKey { .. } => "Revoke",
            Self::DeleteKey { .. } => "Delete Forever",
        }
    }
}

// ─── App state machine ────────────────────────────────────────────────────────

#[derive(PartialEq)] enum Mode { Setup, Connecting, Login, Main }
#[derive(PartialEq, Clone, Copy)] enum Tab { Dashboard, Keys, Metrics, Jobs, System }

struct App {
    rt:    Arc<Runtime>,
    pool:  Option<Arc<PgPool>>,
    http:  Client,
    // Config
    db_url:   String,
    api_base: String,
    secret:   String,
    // State machine
    mode:  Mode,
    // Setup
    in_db:    String,
    in_api:   String,
    conn_err: Option<String>,
    pool_rx:  Option<std::sync::mpsc::Receiver<Result<PgPool, String>>>,
    // Login
    in_sec:    String,
    login_err: Option<String>,
    // Main
    tab:      Tab,
    prev_tab: Option<Tab>,
    shared:   Arc<Mutex<Shared>>,
    // Auto-refresh
    auto_refresh:   bool,
    refresh_secs:   f32,
    last_refresh:   Option<Instant>,
    // Key form
    new_key_open: bool,
    new_name:     String,
    new_quota:    String,
    // Inline quota edit: (key_id, draft_quota)
    editing_quota: Option<(String, String)>,
    // Metrics
    met_days: i64,
    // Confirmation modal
    confirm: Option<Confirm>,
    // Toast: (message, is_error, timestamp)
    toast: Option<(String, bool, Instant)>,
}

impl App {
    fn new(rt: Arc<Runtime>, _cc: &eframe::CreationContext) -> Self {
        let _ = dotenv();
        let db_url   = std::env::var("DATABASE_URL").unwrap_or_default();
        let api_base = std::env::var("API_BASE_URL")
            .unwrap_or_else(|_| "https://jamg-avagen.hf.space".into());
        let secret   = std::env::var("ADMIN_SECRET").unwrap_or_default();

        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap();

        let mut app = Self {
            rt, pool: None, http,
            db_url: db_url.clone(), api_base: api_base.clone(), secret,
            mode:     Mode::Setup,
            in_db:    db_url.clone(),
            in_api:   api_base,
            conn_err: None, pool_rx: None,
            in_sec: String::new(), login_err: None,
            tab: Tab::Dashboard, prev_tab: None,
            shared: Arc::new(Mutex::new(Shared::default())),
            auto_refresh: true, refresh_secs: 30.0, last_refresh: None,
            new_key_open: false, new_name: String::new(), new_quota: "500".into(),
            editing_quota: None,
            met_days: 30,
            confirm: None,
            toast: None,
        };
        if !db_url.is_empty() { app.start_connect(_cc.egui_ctx.clone()); }
        app
    }

    // ── Connection ────────────────────────────────────────────────────────────

    fn start_connect(&mut self, ctx: egui::Context) {
        let url = self.db_url.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.pool_rx  = Some(rx);
        self.mode     = Mode::Connecting;
        self.conn_err = None;
        self.rt.spawn(async move {
            let r = PgPoolOptions::new()
                .max_connections(3)
                .acquire_timeout(Duration::from_secs(15))
                .connect(&url).await
                .map_err(|e| e.to_string());
            let _ = tx.send(r);
            ctx.request_repaint();
        });
    }

    fn poll_connect(&mut self) {
        if let Some(rx) = &self.pool_rx {
            match rx.try_recv() {
                Ok(Ok(pool)) => {
                    self.pool    = Some(Arc::new(pool));
                    self.pool_rx = None;
                    self.mode    = if self.secret.is_empty() { Mode::Login } else { Mode::Main };
                }
                Ok(Err(e)) => {
                    self.conn_err = Some(e);
                    self.pool_rx  = None;
                    self.mode     = Mode::Setup;
                }
                Err(_) => {}
            }
        }
    }

    // ── Data loaders ──────────────────────────────────────────────────────────

    fn load_tab(&self, ctx: &egui::Context) {
        match self.tab {
            Tab::Dashboard => self.load_dash(ctx.clone()),
            Tab::Keys      => self.load_keys(ctx.clone()),
            Tab::Metrics   => self.load_metrics(ctx.clone(), self.met_days),
            Tab::Jobs      => self.load_jobs(ctx.clone()),
            Tab::System    => self.load_sys(ctx.clone()),
        }
    }

    fn load_dash(&self, ctx: egui::Context) {
        let Some(pool) = self.pool.clone() else { return };
        let shared = self.shared.clone();
        let http = self.http.clone(); let api = self.api_base.clone();
        { let mut s = shared.lock().unwrap(); if s.dash_load { return; } s.dash_load = true; }
        self.rt.spawn(async move {
            let d = fetch_dash(&pool, &http, &api).await;
            { let mut s = shared.lock().unwrap(); s.dash = Some(d); s.dash_load = false; }
            ctx.request_repaint();
        });
    }

    fn load_keys(&self, ctx: egui::Context) {
        let Some(pool) = self.pool.clone() else { return };
        let shared = self.shared.clone();
        { let mut s = shared.lock().unwrap(); if s.keys_load { return; } s.keys_load = true; }
        self.rt.spawn(async move {
            match fetch_keys(&pool).await {
                Ok(k)  => { let mut s = shared.lock().unwrap(); s.keys = k; s.keys_err = None; s.keys_load = false; }
                Err(e) => { let mut s = shared.lock().unwrap(); s.keys_err = Some(e.to_string()); s.keys_load = false; }
            }
            ctx.request_repaint();
        });
    }

    fn load_metrics(&self, ctx: egui::Context, days: i64) {
        let Some(pool) = self.pool.clone() else { return };
        let shared = self.shared.clone();
        { let mut s = shared.lock().unwrap(); if s.met_load { return; } s.met_load = true; }
        self.rt.spawn(async move {
            let m = fetch_metrics(&pool, days).await;
            { let mut s = shared.lock().unwrap(); s.metrics = Some(m); s.met_load = false; }
            ctx.request_repaint();
        });
    }

    fn load_jobs(&self, ctx: egui::Context) {
        let Some(pool) = self.pool.clone() else { return };
        let shared = self.shared.clone();
        { let mut s = shared.lock().unwrap(); if s.jobs_load { return; } s.jobs_load = true; }
        self.rt.spawn(async move {
            let j = fetch_jobs(&pool).await;
            { let mut s = shared.lock().unwrap(); s.jobs = j; s.jobs_load = false; }
            ctx.request_repaint();
        });
    }

    fn load_sys(&self, ctx: egui::Context) {
        let Some(pool) = self.pool.clone() else { return };
        let shared = self.shared.clone(); let http = self.http.clone(); let api = self.api_base.clone();
        { let mut s = shared.lock().unwrap(); if s.sys_load { return; } s.sys_load = true; }
        self.rt.spawn(async move {
            let sy = fetch_sys(&pool, &http, &api).await;
            { let mut s = shared.lock().unwrap(); s.sys = Some(sy); s.sys_load = false; }
            ctx.request_repaint();
        });
    }

    // ── Actions ───────────────────────────────────────────────────────────────

    fn exec_confirm(&mut self, ctx: &egui::Context) {
        let Some(action) = self.confirm.take() else { return };
        let pool = self.pool.clone().unwrap();
        let shared = self.shared.clone();
        let ctx2 = ctx.clone();

        match action {
            Confirm::CancelJob { id, .. } => {
                self.rt.spawn(async move {
                    let _ = sqlx::query(
                        "UPDATE batch_jobs SET state='cancelled', updated_at=NOW() \
                         WHERE id=$1 AND state IN ('queued','running','uploading')"
                    ).bind(id).execute(&*pool).await;
                    let jobs = fetch_jobs(&pool).await;
                    let mut s = shared.lock().unwrap(); s.jobs = jobs; s.jobs_load = false;
                    ctx2.request_repaint();
                });
                self.toast = Some(("Job cancellation requested".into(), false, Instant::now()));
            }
            Confirm::DeleteJob { id, .. } => {
                self.rt.spawn(async move {
                    let _ = sqlx::query("DELETE FROM batch_jobs WHERE id=$1")
                        .bind(id).execute(&*pool).await;
                    let jobs = fetch_jobs(&pool).await;
                    let mut s = shared.lock().unwrap(); s.jobs = jobs; s.jobs_load = false;
                    ctx2.request_repaint();
                });
                self.toast = Some(("Job record deleted".into(), false, Instant::now()));
            }
            Confirm::RevokeKey { id, .. } => {
                let id2 = id.clone();
                self.rt.spawn(async move {
                    let _ = sqlx::query(
                        "UPDATE api_keys SET is_active=FALSE, updated_at=NOW() WHERE id=$1"
                    ).bind(&id2).execute(&*pool).await;
                    let keys = fetch_keys(&pool).await.unwrap_or_default();
                    let mut s = shared.lock().unwrap(); s.keys = keys; s.keys_load = false;
                    ctx2.request_repaint();
                });
                self.toast = Some(("Key revoked".into(), false, Instant::now()));
            }
            Confirm::DeleteKey { id, .. } => {
                let id2 = id.clone();
                self.rt.spawn(async move {
                    let _ = sqlx::query("DELETE FROM usage_log WHERE api_key_id=$1")
                        .bind(&id2).execute(&*pool).await;
                    let _ = sqlx::query("DELETE FROM api_keys WHERE id=$1")
                        .bind(&id2).execute(&*pool).await;
                    let keys = fetch_keys(&pool).await.unwrap_or_default();
                    let mut s = shared.lock().unwrap(); s.keys = keys; s.keys_load = false;
                    ctx2.request_repaint();
                });
                self.toast = Some(("Key permanently deleted".into(), false, Instant::now()));
            }
        }
    }

    // ── Screens ───────────────────────────────────────────────────────────────

    fn screen_setup(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(Frame::none().fill(c_bg_panel()))
            .show(ctx, |ui| {
                ui.add_space(60.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("⬡").size(40.0).color(c_accent()));
                    ui.add_space(8.0);
                    ui.label(RichText::new("AvaGen Admin").size(28.0).strong().color(c_text()));
                    ui.label(RichText::new("Connect to your deployment").color(c_text_dim()));
                    ui.add_space(28.0);

                    card_frame().show(ui, |ui| {
                        ui.set_width(460.0);
                        ui.spacing_mut().item_spacing.y = 8.0;

                        field_label(ui, "Database URL");
                        ui.add(TextEdit::singleline(&mut self.in_db)
                            .hint_text("postgres://user:pass@host/db")
                            .desired_width(f32::INFINITY));

                        ui.add_space(6.0);
                        field_label(ui, "API Base URL");
                        ui.add(TextEdit::singleline(&mut self.in_api)
                            .hint_text("https://jamg-avagen.hf.space")
                            .desired_width(f32::INFINITY));

                        ui.add_space(14.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if primary_btn(ui, "Connect →").clicked() {
                                self.db_url   = self.in_db.trim().to_string();
                                self.api_base = self.in_api.trim().to_string();
                                self.start_connect(ctx.clone());
                            }
                        });
                        if let Some(e) = &self.conn_err.clone() {
                            ui.add_space(6.0);
                            ui.label(RichText::new(format!("⊗  {e}")).color(c_danger()).size(13.0));
                        }
                    });
                });
            });
    }

    fn screen_connecting(&self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(Frame::none().fill(c_bg_panel()))
            .show(ctx, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.vertical_centered(|ui| {
                        ui.spinner();
                        ui.add_space(12.0);
                        ui.label(RichText::new("Connecting…").color(c_text_dim()));
                    });
                });
            });
    }

    fn screen_login(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(Frame::none().fill(c_bg_panel()))
            .show(ctx, |ui| {
                ui.add_space(80.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("⬡").size(36.0).color(c_accent()));
                    ui.add_space(6.0);
                    ui.label(RichText::new("AvaGen Admin").size(26.0).strong().color(c_text()));
                    ui.add_space(24.0);
                    card_frame().show(ui, |ui| {
                        ui.set_width(340.0);
                        ui.spacing_mut().item_spacing.y = 8.0;
                        field_label(ui, "Admin Secret");
                        let resp = ui.add(
                            TextEdit::singleline(&mut self.in_sec)
                                .password(true)
                                .desired_width(f32::INFINITY)
                        );
                        ui.add_space(10.0);
                        let enter = resp.lost_focus()
                            && ctx.input(|i| i.key_pressed(egui::Key::Enter));
                        if primary_btn(ui, "Sign In").clicked() || enter { self.do_login(); }
                        if let Some(e) = &self.login_err.clone() {
                            ui.add_space(6.0);
                            ui.label(RichText::new(format!("⊗  {e}")).color(c_danger()).size(13.0));
                        }
                    });
                });
            });
    }

    fn do_login(&mut self) {
        if self.in_sec.is_empty() {
            self.login_err = Some("Enter the admin secret".into()); return;
        }
        if self.secret.is_empty() || self.in_sec == self.secret {
            self.secret    = self.in_sec.clone();
            self.login_err = None;
            self.mode      = Mode::Main;
        } else {
            self.login_err = Some("Incorrect secret".into());
            self.in_sec.clear();
        }
    }

    // ── Main layout ───────────────────────────────────────────────────────────

    fn screen_main(&mut self, ctx: &egui::Context) {
        // Sidebar
        egui::SidePanel::left("sidebar")
            .exact_width(190.0)
            .frame(Frame::none().fill(c_bg_side()).stroke(Stroke::new(1.0, c_border())))
            .show(ctx, |ui| {
                self.render_sidebar(ui, ctx);
            });

        // Toast strip
        let toast = self.toast.clone();
        if let Some((msg, is_err, t)) = toast {
            if t.elapsed() < Duration::from_secs(10) {
                let (bg, fg) = if is_err {
                    (Color32::from_rgb(50, 15, 15), c_danger())
                } else {
                    (Color32::from_rgb(10, 40, 20), c_success())
                };
                egui::TopBottomPanel::top("toast")
                    .exact_height(32.0)
                    .frame(Frame::none().fill(bg))
                    .show(ctx, |ui| {
                        ui.horizontal_centered(|ui| {
                            ui.label(RichText::new(&msg).color(fg).size(13.0));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("✕").clicked() { self.toast = None; }
                            });
                        });
                    });
            } else {
                self.toast = None;
            }
        }

        // Confirmation modal
        if self.confirm.is_some() {
            self.render_confirm(ctx);
        }

        egui::CentralPanel::default()
            .frame(Frame::none().fill(c_bg_panel()))
            .show(ctx, |ui| {
                match self.tab {
                    Tab::Dashboard => self.tab_dashboard(ui, ctx),
                    Tab::Keys      => self.tab_keys(ui, ctx),
                    Tab::Metrics   => self.tab_metrics(ui, ctx),
                    Tab::Jobs      => self.tab_jobs(ui, ctx),
                    Tab::System    => self.tab_system(ui, ctx),
                }
            });
    }

    fn render_sidebar(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.label(RichText::new("⬡ AvaGen").size(17.0).strong().color(c_accent()));
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.label(RichText::new("Admin Console").size(11.0).color(c_text_faint()));
        });

        ui.add_space(18.0);
        ui.add(egui::Separator::default().horizontal().shrink(16.0));
        ui.add_space(8.0);

        for (icon, label, t) in [
            ("◈", "Dashboard", Tab::Dashboard),
            ("⊞", "API Keys",  Tab::Keys),
            ("∿", "Metrics",   Tab::Metrics),
            ("⊙", "Jobs",      Tab::Jobs),
            ("⊟", "System",    Tab::System),
        ] {
            let selected = self.tab == t;
            let (text_c, bg) = if selected {
                (c_accent(), Color32::from_rgba_premultiplied(99, 102, 241, 25))
            } else {
                (c_text_dim(), Color32::TRANSPARENT)
            };
            let btn = egui::Button::new(
                    RichText::new(format!("  {icon}  {label}")).size(13.0).color(text_c)
                )
                .fill(bg)
                .stroke(Stroke::NONE)
                .rounding(Rounding::same(6.0))
                .min_size(Vec2::new(162.0, 32.0));

            if ui.add(btn).clicked() { self.tab = t; }
            ui.add_space(2.0);
        }

        ui.add_space(12.0);
        ui.add(egui::Separator::default().horizontal().shrink(16.0));
        ui.add_space(10.0);

        // Auto-refresh controls
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(RichText::new("Auto-refresh").size(11.0).color(c_text_faint()));
        });
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            let ar = self.auto_refresh;
            let label = if ar { "● ON" } else { "○ OFF" };
            let color = if ar { c_success() } else { c_text_faint() };
            if ui.toggle_value(&mut self.auto_refresh, RichText::new(label).size(12.0).color(color)).clicked() {}
            if self.auto_refresh {
                ui.add(egui::DragValue::new(&mut self.refresh_secs)
                    .range(5.0..=300.0_f32)
                    .speed(1.0)
                    .suffix("s")
                    .max_decimals(0));
            }
        });

        if self.auto_refresh {
            if let Some(lr) = self.last_refresh {
                let elapsed = lr.elapsed().as_secs_f32();
                let remaining = (self.refresh_secs - elapsed).max(0.0) as u32;
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.label(RichText::new(format!("↻ {remaining}s")).size(11.0).color(c_text_faint()));
                });
                let pct = (elapsed / self.refresh_secs).clamp(0.0, 1.0);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    let bar = egui::ProgressBar::new(pct)
                        .desired_width(140.0)
                        .fill(c_accent_dim());
                    ui.add(bar);
                });
            }
        }

        ui.add_space(10.0);
        if ui.horizontal(|ui| {
            ui.add_space(14.0);
            ghost_btn(ui, "⟳  Refresh Now").clicked()
        }).inner {
            self.invalidate_tab();
            self.load_tab(ctx);
            self.last_refresh = Some(Instant::now());
        }

        // Spacer
        let avail = ui.available_height();
        ui.add_space(avail - 52.0);
        ui.add(egui::Separator::default().horizontal().shrink(16.0));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            if ghost_btn(ui, "⎋  Logout").clicked() {
                *self.shared.lock().unwrap() = Shared::default();
                self.pool = None;
                self.mode = Mode::Setup;
                self.in_sec.clear();
            }
        });
        ui.add_space(8.0);
    }

    fn invalidate_tab(&mut self) {
        let mut s = self.shared.lock().unwrap();
        match self.tab {
            Tab::Dashboard => s.dash    = None,
            Tab::Keys      => { s.keys.clear(); s.keys_err = None; }
            Tab::Metrics   => s.metrics = None,
            Tab::Jobs      => s.jobs.clear(),
            Tab::System    => s.sys     = None,
        }
    }

    fn render_confirm(&mut self, ctx: &egui::Context) {
        let confirm = match &self.confirm { Some(c) => c.clone(), None => return };
        let mut open = true;

        egui::Window::new(confirm.title())
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(Frame::none().fill(c_bg_card()).stroke(Stroke::new(1.0, c_border_hi())).rounding(10.0).inner_margin(24.0))
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_width(340.0);
                ui.label(RichText::new(confirm.message()).color(c_text_dim()).size(13.0));
                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    if ghost_btn(ui, "Cancel").clicked() { self.confirm = None; }
                    ui.add_space(8.0);
                    if danger_btn(ui, confirm.confirm_label()).clicked() {
                        self.exec_confirm(ctx);
                    }
                });
            });

        if !open { self.confirm = None; }
    }

    // ── Tab: Dashboard ────────────────────────────────────────────────────────

    fn tab_dashboard(&self, ui: &mut Ui, ctx: &egui::Context) {
        let need = { let s = self.shared.lock().unwrap(); s.dash.is_none() && !s.dash_load };
        if need { self.load_dash(ctx.clone()); }

        let (loading, dash) = { let s = self.shared.lock().unwrap(); (s.dash_load, s.dash.clone()) };
        if loading && dash.is_none() { loading_center(ui); return; }
        let dash = match dash { Some(d) => d, None => return };

        tab_scroll(ui, |ui| {
            section_header(ui, "Overview");
            ui.add_space(10.0);

            ui.horizontal_wrapped(|ui| {
                stat_card(ui, "Total Keys",  &dash.total_keys.to_string(),  c_accent());
                stat_card(ui, "Active Keys", &dash.active_keys.to_string(), c_success());
                stat_card(ui, "Today",       &dash.today_req.to_string(),   c_text_dim());
                stat_card(ui, "This Month",  &dash.month_req.to_string(),   c_text_dim());
                stat_card(ui, "Active Jobs", &dash.active_jobs.to_string(), c_warning());
                let (hc, hl) = match dash.health.as_str() {
                    "online"   => (c_success(), "● Online"),
                    "degraded" => (c_warning(), "◐ Degraded"),
                    _          => (c_danger(),  "○ Offline"),
                };
                stat_card_label(ui, "API Status", hl, hc);
            });

            ui.add_space(20.0);
            section_header(ui, "Recent Activity");
            ui.add_space(8.0);

            card_frame().show(ui, |ui| {
                TableBuilder::new(ui).striped(true).resizable(true)
                    .column(Column::auto().at_least(140.0))
                    .column(Column::auto().at_least(80.0))
                    .column(Column::auto().at_least(200.0))
                    .column(Column::auto().at_least(60.0))
                    .column(Column::remainder().at_least(150.0))
                    .header(24.0, |mut h| {
                        h.col(|ui| { th(ui, "Key Name"); });
                        h.col(|ui| { th(ui, "Prefix"); });
                        h.col(|ui| { th(ui, "Endpoint"); });
                        h.col(|ui| { th(ui, "Count"); });
                        h.col(|ui| { th(ui, "Time (UTC)"); });
                    })
                    .body(|mut body| {
                        for r in &dash.recent {
                            body.row(20.0, |mut row| {
                                row.col(|ui| { td(ui, &r.name); });
                                row.col(|ui| { td_mono(ui, &r.prefix); });
                                row.col(|ui| { td_mono(ui, &r.endpoint); });
                                row.col(|ui| { td(ui, &r.count.to_string()); });
                                row.col(|ui| { td_dim(ui, &r.when); });
                            });
                        }
                    });
            });
        });
    }

    // ── Tab: API Keys ─────────────────────────────────────────────────────────

    fn tab_keys(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let need = { let s = self.shared.lock().unwrap(); s.keys.is_empty() && !s.keys_load };
        if need { self.load_keys(ctx.clone()); }

        tab_scroll(ui, |ui| {
            section_header(ui, "API Keys");
            ui.add_space(10.0);

            // Create key form
            let hdr_text = if self.new_key_open { "▾  New Key" } else { "▸  New Key" };
            if ghost_btn(ui, hdr_text).clicked() { self.new_key_open = !self.new_key_open; }
            if self.new_key_open {
                ui.add_space(6.0);
                subtle_frame().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        field_label(ui, "Name:");
                        ui.add(TextEdit::singleline(&mut self.new_name)
                            .hint_text("e.g. MyApp").desired_width(200.0));
                        ui.add_space(8.0);
                        field_label(ui, "Monthly Quota:");
                        ui.add(TextEdit::singleline(&mut self.new_quota).desired_width(80.0));
                        ui.add_space(8.0);
                        let can = !self.new_name.trim().is_empty();
                        ui.add_enabled_ui(can, |ui| {
                            if primary_btn(ui, "Create Key").clicked() {
                                let name  = self.new_name.trim().to_string();
                                let quota = self.new_quota.trim().parse::<i64>().unwrap_or(500);
                                let ctx2  = ctx.clone();
                                let shd   = self.shared.clone();
                                let pool  = self.pool.clone().unwrap();
                                let http  = self.http.clone();
                                let api   = self.api_base.clone();
                                let sec   = self.secret.clone();
                                self.rt.spawn(async move {
                                    let r = http.post(format!("{api}/api/admin/keys"))
                                        .header("X-Admin-Secret", &sec)
                                        .json(&json!({ "name": name, "monthly_quota": quota }))
                                        .send().await;
                                    let _ = match r {
                                        Ok(resp) if resp.status().is_success() => {
                                            let b: Value = resp.json().await.unwrap_or_default();
                                            let _key = b.get("key")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("(see logs)");
                                        }
                                        _ => {}
                                    };
                                    let keys = fetch_keys(&pool).await.unwrap_or_default();
                                    let mut s = shd.lock().unwrap();
                                    s.keys = keys; s.keys_load = false;
                                    ctx2.request_repaint();
                                });
                                self.new_name.clear();
                                self.new_key_open = false;
                                self.toast = Some(("Key creation requested — check the API response for the raw key".into(), false, Instant::now()));
                            }
                        });
                    });
                });
                ui.add_space(8.0);
            }

            let (loading, keys, err) = {
                let s = self.shared.lock().unwrap();
                (s.keys_load, s.keys.clone(), s.keys_err.clone())
            };
            if loading && keys.is_empty() { loading_center(ui); return; }
            if let Some(e) = err { ui.label(RichText::new(e).color(c_danger())); return; }

            ui.add_space(6.0);
            card_frame().show(ui, |ui| {
                let mut quota_action: Option<(String, String)> = None;
                let mut confirm_action: Option<Confirm> = None;

                TableBuilder::new(ui).striped(true).resizable(true)
                    .column(Column::auto().at_least(150.0))
                    .column(Column::auto().at_least(80.0))
                    .column(Column::auto().at_least(130.0))
                    .column(Column::auto().at_least(80.0))
                    .column(Column::auto().at_least(80.0))
                    .column(Column::auto().at_least(80.0))
                    .column(Column::auto().at_least(90.0))
                    .column(Column::remainder().at_least(170.0))
                    .header(24.0, |mut h| {
                        h.col(|ui| { th(ui, "Name"); });
                        h.col(|ui| { th(ui, "Prefix"); });
                        h.col(|ui| { th(ui, "Monthly Used"); });
                        h.col(|ui| { th(ui, "Quota"); });
                        h.col(|ui| { th(ui, "Total Used"); });
                        h.col(|ui| { th(ui, "Status"); });
                        h.col(|ui| { th(ui, "Created"); });
                        h.col(|ui| { th(ui, "Actions"); });
                    })
                    .body(|mut body| {
                        for k in &keys {
                            let k = k.clone();
                            let pct = if k.monthly_quota > 0 {
                                k.monthly_used * 100 / k.monthly_quota
                            } else { 0 };
                            let editing_this = self.editing_quota.as_ref()
                                .map(|(eid, _)| eid == &k.id)
                                .unwrap_or(false);

                            body.row(28.0, |mut row| {
                                row.col(|ui| { td(ui, &k.name); });
                                row.col(|ui| { td_mono(ui, &k.prefix); });
                                row.col(|ui| {
                                    let c = if pct > 90 { c_danger() }
                                        else if pct > 70 { c_warning() }
                                        else { c_text_dim() };
                                    ui.label(RichText::new(format!("{} ({}%)", k.monthly_used, pct)).color(c).size(12.0));
                                });
                                row.col(|ui| {
                                    if editing_this {
                                        let mut qs = self.editing_quota.as_ref().map(|(_, v)| v.clone()).unwrap_or_default();
                                        let r = ui.add(TextEdit::singleline(&mut qs).desired_width(60.0));
                                        if let Some((_, ref mut v)) = self.editing_quota { *v = qs.clone(); }
                                        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                            quota_action = Some((k.id.clone(), qs));
                                        }
                                    } else {
                                        td(ui, &k.monthly_quota.to_string());
                                    }
                                });
                                row.col(|ui| { td(ui, &k.total_used.to_string()); });
                                row.col(|ui| {
                                    let (t, c) = if k.is_active { ("● Active", c_success()) } else { ("○ Revoked", c_text_faint()) };
                                    ui.label(RichText::new(t).color(c).size(12.0));
                                });
                                row.col(|ui| { td_dim(ui, &k.created_at); });
                                row.col(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 4.0;
                                        if editing_this {
                                            if accent_btn(ui, "Save").clicked() {
                                                let qs = self.editing_quota.as_ref().map(|(_, v)| v.clone()).unwrap_or_default();
                                                quota_action = Some((k.id.clone(), qs));
                                            }
                                            if ghost_btn(ui, "✕").clicked() { self.editing_quota = None; }
                                        } else {
                                            if ghost_btn(ui, "✎").on_hover_text("Edit quota").clicked() {
                                                self.editing_quota = Some((k.id.clone(), k.monthly_quota.to_string()));
                                            }
                                            if k.is_active {
                                                if warn_btn(ui, "Revoke").clicked() {
                                                    confirm_action = Some(Confirm::RevokeKey { id: k.id.clone(), name: k.name.clone() });
                                                }
                                            }
                                            if danger_btn(ui, "Delete").clicked() {
                                                confirm_action = Some(Confirm::DeleteKey { id: k.id.clone(), name: k.name.clone() });
                                            }
                                        }
                                    });
                                });
                            });
                        }
                    });

                if let Some((id, qs)) = quota_action {
                    if let Ok(quota) = qs.parse::<i64>() {
                        let pool = self.pool.clone().unwrap();
                        let shared = self.shared.clone();
                        let ctx2 = ctx.clone();
                        self.rt.spawn(async move {
                            let _ = sqlx::query("UPDATE api_keys SET monthly_quota=$2, updated_at=NOW() WHERE id=$1")
                                .bind(&id).bind(quota).execute(&*pool).await;
                            let keys = fetch_keys(&pool).await.unwrap_or_default();
                            let mut s = shared.lock().unwrap(); s.keys = keys; s.keys_load = false;
                            ctx2.request_repaint();
                        });
                        self.editing_quota = None;
                        self.toast = Some(("Quota updated".into(), false, Instant::now()));
                    }
                }
                if let Some(ca) = confirm_action { self.confirm = Some(ca); }
            });
        });
    }

    // ── Tab: Metrics ──────────────────────────────────────────────────────────

    fn tab_metrics(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let need = { let s = self.shared.lock().unwrap(); s.metrics.is_none() && !s.met_load };
        if need { self.load_metrics(ctx.clone(), self.met_days); }

        let (loading, metrics) = { let s = self.shared.lock().unwrap(); (s.met_load, s.metrics.clone()) };

        tab_scroll(ui, |ui| {
            section_header(ui, "Metrics");
            ui.horizontal(|ui| {
                ui.label(RichText::new("Window:").color(c_text_dim()).size(12.0));
                ui.add(egui::DragValue::new(&mut self.met_days)
                    .range(1..=365_i64).speed(1.0).suffix(" days").max_decimals(0));
                if accent_btn(ui, "Apply").clicked() {
                    self.shared.lock().unwrap().metrics = None;
                    self.load_metrics(ctx.clone(), self.met_days);
                }
            });
            ui.add_space(12.0);

            if loading && metrics.is_none() { loading_center(ui); return; }
            let m = match metrics { Some(m) => m, None => return };

            // Daily chart
            subsection(ui, &format!("Daily Requests — last {} days", self.met_days));
            if !m.daily.is_empty() {
                card_frame().show(ui, |ui| {
                    let nd = m.daily.len();
                    let bars: Vec<Bar> = m.daily.iter().enumerate()
                        .map(|(i, (_, v))| Bar::new(i as f64, *v).width(0.8))
                        .collect();
                    Plot::new("daily").height(170.0)
                        .allow_drag(false).allow_zoom(false)
                        .show_background(false)
                        .show(ui, |p| { p.bar_chart(BarChart::new(bars).color(c_accent())); });
                    ui.horizontal(|ui| {
                        td_dim(ui, &m.daily[0].0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            td_dim(ui, &m.daily[nd - 1].0);
                        });
                    });
                });
            } else {
                ui.label(RichText::new("No data for this period.").color(c_text_faint()));
            }

            ui.add_space(14.0);
            subsection(ui, "Hourly Requests (today)");
            card_frame().show(ui, |ui| {
                if !m.hourly.is_empty() {
                    let bars: Vec<Bar> = m.hourly.iter()
                        .map(|(h, v)| Bar::new(*h as f64, *v).width(0.8)).collect();
                    Plot::new("hourly").height(120.0)
                        .allow_drag(false).allow_zoom(false)
                        .show_background(false)
                        .show(ui, |p| { p.bar_chart(BarChart::new(bars).color(c_success())); });
                } else {
                    ui.label(RichText::new("No requests today yet.").color(c_text_faint()));
                }
            });

            ui.add_space(14.0);
            subsection(ui, "Usage by Key — this month");
            card_frame().show(ui, |ui| {
                if !m.per_key.is_empty() {
                    TableBuilder::new(ui).striped(true)
                        .column(Column::auto().at_least(180.0))
                        .column(Column::auto().at_least(70.0))
                        .column(Column::auto().at_least(70.0))
                        .column(Column::remainder().at_least(160.0))
                        .header(22.0, |mut h| {
                            h.col(|ui| { th(ui, "Key"); });
                            h.col(|ui| { th(ui, "Used"); });
                            h.col(|ui| { th(ui, "Quota"); });
                            h.col(|ui| { th(ui, ""); });
                        })
                        .body(|mut body| {
                            for (name, used, quota) in &m.per_key {
                                body.row(20.0, |mut row| {
                                    row.col(|ui| { td(ui, name); });
                                    row.col(|ui| { td(ui, &used.to_string()); });
                                    row.col(|ui| { td(ui, &quota.to_string()); });
                                    row.col(|ui| {
                                        let p = if *quota > 0 { *used as f32 / *quota as f32 } else { 0.0 };
                                        let p = p.clamp(0.0, 1.0);
                                        let fill = if p > 0.9 { c_danger() } else if p > 0.7 { c_warning() } else { c_accent() };
                                        ui.add(egui::ProgressBar::new(p).fill(fill).show_percentage());
                                    });
                                });
                            }
                        });
                } else { ui.label(RichText::new("No active keys.").color(c_text_faint())); }
            });

            ui.add_space(14.0);
            subsection(ui, "Top Endpoints — 30 days");
            card_frame().show(ui, |ui| {
                if !m.endpoints.is_empty() {
                    let max = m.endpoints.iter().map(|(_, n)| *n).max().unwrap_or(1);
                    TableBuilder::new(ui).striped(true)
                        .column(Column::auto().at_least(260.0))
                        .column(Column::auto().at_least(80.0))
                        .column(Column::remainder())
                        .header(22.0, |mut h| {
                            h.col(|ui| { th(ui, "Endpoint"); });
                            h.col(|ui| { th(ui, "Requests"); });
                            h.col(|ui| { th(ui, ""); });
                        })
                        .body(|mut body| {
                            for (ep, n) in &m.endpoints {
                                body.row(20.0, |mut row| {
                                    row.col(|ui| { td_mono(ui, ep); });
                                    row.col(|ui| { td(ui, &n.to_string()); });
                                    row.col(|ui| {
                                        let p = *n as f32 / max as f32;
                                        ui.add(egui::ProgressBar::new(p).fill(c_accent_dim()));
                                    });
                                });
                            }
                        });
                } else { ui.label(RichText::new("No data.").color(c_text_faint())); }
            });
        });
    }

    // ── Tab: Jobs ─────────────────────────────────────────────────────────────

    fn tab_jobs(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let need = { let s = self.shared.lock().unwrap(); s.jobs.is_empty() && !s.jobs_load };
        if need { self.load_jobs(ctx.clone()); }

        let (loading, jobs) = { let s = self.shared.lock().unwrap(); (s.jobs_load, s.jobs.clone()) };
        if loading && jobs.is_empty() { loading_center(ui); return; }

        let active_count = jobs.iter().filter(|j| j.active).count();
        let mut confirm_action: Option<Confirm> = None;

        tab_scroll(ui, |ui| {
            ui.horizontal(|ui| {
                section_header(ui, "Batch Jobs");
                ui.add_space(8.0);
                badge(ui, &format!("{} total", jobs.len()), c_text_faint());
                badge(ui, &format!("{} active", active_count), c_warning());
            });
            ui.add_space(10.0);

            card_frame().show(ui, |ui| {
                TableBuilder::new(ui).striped(true).resizable(true)
                    .column(Column::auto().at_least(100.0))
                    .column(Column::auto().at_least(80.0))
                    .column(Column::auto().at_least(100.0))
                    .column(Column::auto().at_least(55.0))
                    .column(Column::auto().at_least(75.0))
                    .column(Column::auto().at_least(120.0))
                    .column(Column::auto().at_least(55.0))
                    .column(Column::auto().at_least(130.0))
                    .column(Column::remainder().at_least(140.0))
                    .header(24.0, |mut h| {
                        h.col(|ui| { th(ui, "Job ID"); });
                        h.col(|ui| { th(ui, "State"); });
                        h.col(|ui| { th(ui, "Progress"); });
                        h.col(|ui| { th(ui, "Failed"); });
                        h.col(|ui| { th(ui, "Model"); });
                        h.col(|ui| { th(ui, "API Key"); });
                        h.col(|ui| { th(ui, "Queue"); });
                        h.col(|ui| { th(ui, "Created"); });
                        h.col(|ui| { th(ui, "Actions"); });
                    })
                    .body(|mut body| {
                        for j in &jobs {
                            let j = j.clone();
                            let pct = if j.total > 0 { j.done as f32 / j.total as f32 } else { 0.0 };
                            body.row(26.0, |mut row| {
                                row.col(|ui| { td_mono(ui, &j.id_str[..8]); });
                                row.col(|ui| {
                                    let c = job_color(&j.state);
                                    ui.label(RichText::new(job_icon(&j.state)).color(c).size(12.0));
                                });
                                row.col(|ui| {
                                    ui.vertical(|ui| {
                                        ui.add(egui::ProgressBar::new(pct)
                                            .fill(if j.active { c_accent() } else { c_text_faint() })
                                            .desired_width(90.0));
                                        ui.label(RichText::new(format!("{}/{}", j.done, j.total)).size(10.0).color(c_text_dim()));
                                    });
                                });
                                row.col(|ui| { td(ui, &j.failed.to_string()); });
                                row.col(|ui| { td_dim(ui, &j.model); });
                                row.col(|ui| { td_dim(ui, &j.key); });
                                row.col(|ui| { td(ui, &j.qpos); });
                                row.col(|ui| { td_dim(ui, &j.created); });
                                row.col(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 4.0;
                                        if j.active {
                                            if warn_btn(ui, "Cancel").clicked() {
                                                confirm_action = Some(Confirm::CancelJob {
                                                    id: j.id, label: j.id_str[..8].to_string()
                                                });
                                            }
                                        }
                                        if danger_btn(ui, "Delete").clicked() {
                                            confirm_action = Some(Confirm::DeleteJob {
                                                id: j.id, label: j.id_str[..8].to_string()
                                            });
                                        }
                                    });
                                });
                            });
                        }
                    });
            });
        });

        if let Some(ca) = confirm_action { self.confirm = Some(ca); }
    }

    // ── Tab: System ───────────────────────────────────────────────────────────

    fn tab_system(&self, ui: &mut Ui, ctx: &egui::Context) {
        let need = { let s = self.shared.lock().unwrap(); s.sys.is_none() && !s.sys_load };
        if need { self.load_sys(ctx.clone()); }

        let (loading, sys) = { let s = self.shared.lock().unwrap(); (s.sys_load, s.sys.clone()) };
        if loading && sys.is_none() { loading_center(ui); return; }
        let sys = match sys { Some(s) => s, None => return };

        tab_scroll(ui, |ui| {
            section_header(ui, "System");
            ui.add_space(10.0);

            ui.horizontal_wrapped(|ui| {
                // API card
                card_frame().show(ui, |ui| {
                    ui.set_min_width(240.0);
                    let (icon, c) = if sys.api_ok { ("● Online", c_success()) } else { ("○ Offline", c_danger()) };
                    ui.label(RichText::new("API Server").strong().color(c_text_dim()).size(11.0));
                    ui.add_space(4.0);
                    ui.label(RichText::new(icon).color(c).size(18.0).strong());
                    ui.add_space(6.0);
                    ui.label(RichText::new(&sys.api_url).color(c_text_dim()).size(11.0));
                    if !sys.api_err.is_empty() {
                        ui.label(RichText::new(&sys.api_err).color(c_danger()).size(11.0));
                    }
                });
                ui.add_space(10.0);

                // DB card
                card_frame().show(ui, |ui| {
                    ui.set_min_width(240.0);
                    let (icon, c) = if sys.db_ok { ("● Connected", c_success()) } else { ("○ Error", c_danger()) };
                    ui.label(RichText::new("Database").strong().color(c_text_dim()).size(11.0));
                    ui.add_space(4.0);
                    ui.label(RichText::new(icon).color(c).size(18.0).strong());
                    ui.add_space(6.0);
                    ui.label(RichText::new(&sys.db_ver).color(c_text_faint()).size(10.0));
                });
            });

            ui.add_space(14.0);
            subsection(ui, "Database Statistics");
            card_frame().show(ui, |ui| {
                egui::Grid::new("dbstats").num_columns(2).spacing([30.0, 8.0]).striped(true).show(ui, |ui| {
                    kv(ui, "API Keys",        &sys.db_keys.to_string());
                    kv(ui, "Usage Log Rows",  &sys.db_logs.to_string());
                    kv(ui, "Batch Job Rows",  &sys.db_jobs.to_string());
                    kv(ui, "API Base URL",    &sys.api_url);
                });
            });
        });
    }
}

// ─── eframe::App ──────────────────────────────────────────────────────────────

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.mode == Mode::Connecting { self.poll_connect(); }

        // Auto-load when tab changes
        if self.mode == Mode::Main && self.prev_tab != Some(self.tab) {
            self.prev_tab = Some(self.tab);
            let need = {
                let s = self.shared.lock().unwrap();
                match self.tab {
                    Tab::Dashboard => s.dash.is_none()    && !s.dash_load,
                    Tab::Keys      => s.keys.is_empty()   && !s.keys_load,
                    Tab::Metrics   => s.metrics.is_none() && !s.met_load,
                    Tab::Jobs      => s.jobs.is_empty()   && !s.jobs_load,
                    Tab::System    => s.sys.is_none()     && !s.sys_load,
                }
            };
            if need { self.load_tab(ctx); }
            self.last_refresh = Some(Instant::now());
        }

        // Auto-refresh tick
        if self.mode == Mode::Main && self.auto_refresh {
            let should = self.last_refresh
                .map(|t| t.elapsed().as_secs_f32() >= self.refresh_secs)
                .unwrap_or(false);
            if should {
                self.invalidate_tab();
                self.load_tab(ctx);
                self.last_refresh = Some(Instant::now());
            }
        }

        match self.mode {
            Mode::Setup      => self.screen_setup(ctx),
            Mode::Connecting => self.screen_connecting(ctx),
            Mode::Login      => self.screen_login(ctx),
            Mode::Main       => self.screen_main(ctx),
        }

        // Keep animating while busy or counting down
        let busy = { let s = self.shared.lock().unwrap(); s.dash_load || s.keys_load || s.met_load || s.jobs_load || s.sys_load };
        if busy || self.mode == Mode::Connecting {
            ctx.request_repaint_after(Duration::from_millis(80));
        } else if self.auto_refresh && self.mode == Mode::Main {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }
}

// ─── Async DB / HTTP fetchers ─────────────────────────────────────────────────

async fn fetch_dash(pool: &PgPool, http: &Client, api: &str) -> DashData {
    let (total_keys, active_keys): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*)::bigint, COUNT(*) FILTER(WHERE is_active)::bigint FROM api_keys",
    ).fetch_one(pool).await.unwrap_or((0, 0));

    let (today,): (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(count),0)::bigint FROM usage_log WHERE created_at >= CURRENT_DATE",
    ).fetch_one(pool).await.unwrap_or((0,));

    let (month,): (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(count),0)::bigint FROM usage_log WHERE created_at >= DATE_TRUNC('month',NOW())",
    ).fetch_one(pool).await.unwrap_or((0,));

    let (active_jobs,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::bigint FROM batch_jobs WHERE state IN ('queued','running','uploading')",
    ).fetch_one(pool).await.unwrap_or((0,));

    let rows: Vec<DbRecent> = sqlx::query_as(
        "SELECT ak.name, ak.key_prefix, ul.endpoint, ul.count, ul.created_at \
         FROM usage_log ul LEFT JOIN api_keys ak ON ak.id = ul.api_key_id \
         ORDER BY ul.created_at DESC LIMIT 15",
    ).fetch_all(pool).await.unwrap_or_default();

    let health = match http.get(format!("{api}/health")).timeout(Duration::from_secs(4)).send().await {
        Ok(r) if r.status().is_success() => "online".into(),
        Ok(_)  => "degraded".into(),
        Err(_) => "offline".into(),
    };

    DashData {
        total_keys, active_keys, today_req: today, month_req: month, active_jobs, health,
        recent: rows.into_iter().map(|r| UiRecent {
            name:     r.name.unwrap_or_else(|| "(deleted)".into()),
            prefix:   r.key_prefix.unwrap_or_default(),
            endpoint: r.endpoint,
            count:    r.count,
            when:     r.created_at.format("%Y-%m-%d %H:%M").to_string(),
        }).collect(),
    }
}

async fn fetch_keys(pool: &PgPool) -> Result<Vec<UiKey>, sqlx::Error> {
    let rows: Vec<DbKey> = sqlx::query_as(
        "SELECT ak.id, ak.name, ak.key_prefix, ak.monthly_quota, ak.is_active, \
                ak.created_at, ak.updated_at, \
                COALESCE(SUM(ul.count) FILTER(WHERE ul.created_at >= DATE_TRUNC('month',NOW())),0)::bigint AS monthly_used, \
                COALESCE(SUM(ul.count),0)::bigint AS total_used \
         FROM api_keys ak LEFT JOIN usage_log ul ON ul.api_key_id = ak.id \
         GROUP BY ak.id ORDER BY ak.created_at DESC",
    ).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|k| UiKey {
        id: k.id, name: k.name, prefix: k.key_prefix, monthly_quota: k.monthly_quota,
        is_active: k.is_active, monthly_used: k.monthly_used, total_used: k.total_used,
        created_at: k.created_at.format("%Y-%m-%d").to_string(),
    }).collect())
}

async fn fetch_metrics(pool: &PgPool, days: i64) -> MetricsData {
    let daily: Vec<DbDaily> = sqlx::query_as(
        "SELECT DATE(created_at) AS d, COALESCE(SUM(count),0)::bigint AS n \
         FROM usage_log WHERE created_at >= NOW()-($1::bigint*INTERVAL '1 day') \
         GROUP BY d ORDER BY d",
    ).bind(days).fetch_all(pool).await.unwrap_or_default();

    let hourly: Vec<DbHourly> = sqlx::query_as(
        "SELECT EXTRACT(HOUR FROM created_at)::int AS h, COALESCE(SUM(count),0)::bigint AS n \
         FROM usage_log WHERE created_at >= CURRENT_DATE GROUP BY h ORDER BY h",
    ).fetch_all(pool).await.unwrap_or_default();

    let per_key: Vec<DbPerKey> = sqlx::query_as(
        "SELECT ak.name, ak.monthly_quota, COALESCE(SUM(ul.count),0)::bigint AS used \
         FROM api_keys ak LEFT JOIN usage_log ul ON ul.api_key_id=ak.id \
                AND ul.created_at >= DATE_TRUNC('month',NOW()) \
         WHERE ak.is_active=TRUE GROUP BY ak.id,ak.name,ak.monthly_quota ORDER BY used DESC",
    ).fetch_all(pool).await.unwrap_or_default();

    let endpoints: Vec<DbEndpt> = sqlx::query_as(
        "SELECT endpoint, COALESCE(SUM(count),0)::bigint AS n \
         FROM usage_log WHERE created_at >= NOW()-INTERVAL '30 days' \
         GROUP BY endpoint ORDER BY n DESC LIMIT 10",
    ).fetch_all(pool).await.unwrap_or_default();

    MetricsData {
        daily:     daily.into_iter().map(|r| (r.d.to_string(), r.n as f64)).collect(),
        hourly:    hourly.into_iter().map(|r| (r.h, r.n as f64)).collect(),
        per_key:   per_key.into_iter().map(|r| (r.name, r.used, r.monthly_quota)).collect(),
        endpoints: endpoints.into_iter().map(|r| (r.endpoint, r.n)).collect(),
    }
}

async fn fetch_jobs(pool: &PgPool) -> Vec<UiJob> {
    let rows: Vec<DbJob> = sqlx::query_as(
        "SELECT bj.id, bj.state, bj.total, bj.completed, bj.failed_count, bj.model, \
                ak.name AS key_name, \
                CASE WHEN bj.state IN ('queued','running','uploading') THEN \
                    (SELECT COUNT(*) FROM batch_jobs b2 \
                     WHERE b2.state IN ('queued','running','uploading') AND b2.created_at < bj.created_at)+1 \
                ELSE NULL END AS queue_position, \
                bj.created_at \
         FROM batch_jobs bj LEFT JOIN api_keys ak ON ak.id=bj.api_key_id \
         ORDER BY bj.created_at DESC LIMIT 200",
    ).fetch_all(pool).await.unwrap_or_default();

    rows.into_iter().map(|j| {
        let active = matches!(j.state.as_str(), "queued" | "running" | "uploading");
        UiJob {
            id:      j.id,
            id_str:  j.id.to_string(),
            state:   j.state,
            total:   j.total,
            done:    j.completed,
            failed:  j.failed_count,
            model:   j.model,
            key:     j.key_name.unwrap_or_default(),
            qpos:    j.queue_position.map_or("—".into(), |p| p.to_string()),
            created: j.created_at.format("%Y-%m-%d %H:%M").to_string(),
            active,
        }
    }).collect()
}

async fn fetch_sys(pool: &PgPool, http: &Client, api: &str) -> SysData {
    let (api_ok, api_err) = match http.get(format!("{api}/health")).timeout(Duration::from_secs(5)).send().await {
        Ok(r) if r.status().is_success() => (true,  String::new()),
        Ok(r)  => (false, format!("HTTP {}", r.status())),
        Err(e) => (false, e.to_string()),
    };
    let (db_ok, db_ver, db_keys, db_logs, db_jobs) =
        match sqlx::query_as::<_, (String,)>("SELECT version()").fetch_one(pool).await {
            Ok((v,)) => {
                let (k,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM api_keys").fetch_one(pool).await.unwrap_or((0,));
                let (l,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM usage_log").fetch_one(pool).await.unwrap_or((0,));
                let (j,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM batch_jobs").fetch_one(pool).await.unwrap_or((0,));
                (true, v, k, l, j)
            }
            Err(e) => (false, e.to_string(), 0, 0, 0),
        };
    SysData { api_ok, api_err, api_url: api.into(), db_ok, db_ver, db_keys, db_logs, db_jobs }
}

// ─── UI helpers ───────────────────────────────────────────────────────────────

fn setup_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill             = c_bg_panel();
    v.window_fill            = c_bg_card();
    v.faint_bg_color         = c_bg_deep();
    v.extreme_bg_color       = c_bg_deep();
    v.code_bg_color          = c_bg_input();
    v.widgets.noninteractive.bg_fill   = c_bg_card();
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, c_border());
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, c_text_dim());
    v.widgets.inactive.bg_fill         = c_bg_input();
    v.widgets.inactive.bg_stroke       = Stroke::new(1.0, c_border());
    v.widgets.inactive.fg_stroke       = Stroke::new(1.0, c_text_dim());
    v.widgets.hovered.bg_fill          = Color32::from_rgb(38, 52, 85);
    v.widgets.hovered.bg_stroke        = Stroke::new(1.0, c_border_hi());
    v.widgets.hovered.fg_stroke        = Stroke::new(1.5, c_text());
    v.widgets.active.bg_fill           = c_accent();
    v.widgets.active.bg_stroke         = Stroke::new(1.0, c_accent());
    v.widgets.active.fg_stroke         = Stroke::new(1.5, Color32::WHITE);
    v.widgets.open.bg_fill             = c_bg_input();
    v.selection.bg_fill                = Color32::from_rgba_premultiplied(99, 102, 241, 60);
    v.selection.stroke                 = Stroke::new(1.0, c_accent());
    v.window_rounding                  = Rounding::same(10.0);
    v.menu_rounding                    = Rounding::same(6.0);
    v.popup_shadow                     = egui::epaint::Shadow { offset: Vec2::new(0.0, 8.0), blur: 24.0, spread: 0.0, color: Color32::from_black_alpha(80) };
    ctx.set_visuals(v);

    let mut s = (*ctx.style()).clone();
    s.spacing.item_spacing   = Vec2::new(10.0, 6.0);
    s.spacing.button_padding = Vec2::new(10.0, 5.0);
    s.spacing.window_margin  = Margin::same(20.0);
    s.spacing.scroll.bar_width = 6.0;
    ctx.set_style(s);
}

fn tab_scroll(ui: &mut Ui, content: impl FnOnce(&mut Ui)) {
    ui.add_space(4.0);
    ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(12.0);
        ui.horizontal(|ui| { ui.add_space(16.0); ui.vertical(|ui| { content(ui); ui.add_space(20.0); }); ui.add_space(16.0); });
    });
}

fn section_header(ui: &mut Ui, title: &str) {
    ui.label(RichText::new(title).size(20.0).strong().color(c_text()));
    ui.add_space(2.0);
}

fn subsection(ui: &mut Ui, title: &str) {
    ui.label(RichText::new(title).size(13.0).strong().color(c_text_dim()));
    ui.add_space(4.0);
}

fn field_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(c_text_dim()));
}

fn th(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(11.0).strong().color(c_text_faint()).raised());
}
fn td(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(c_text()));
}
fn td_dim(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(11.0).color(c_text_dim()));
}
fn td_mono(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(11.0).color(c_text_dim()).monospace());
}

fn stat_card(ui: &mut Ui, label: &str, value: &str, accent: Color32) {
    card_frame().show(ui, |ui| {
        ui.set_min_width(108.0);
        ui.label(RichText::new(label).size(11.0).color(c_text_faint()));
        ui.add_space(4.0);
        ui.label(RichText::new(value).size(26.0).strong().color(accent));
    });
    ui.add_space(8.0);
}
fn stat_card_label(ui: &mut Ui, label: &str, value: &str, accent: Color32) {
    card_frame().show(ui, |ui| {
        ui.set_min_width(108.0);
        ui.label(RichText::new(label).size(11.0).color(c_text_faint()));
        ui.add_space(4.0);
        ui.label(RichText::new(value).size(15.0).strong().color(accent));
    });
    ui.add_space(8.0);
}

fn badge(ui: &mut Ui, text: &str, c: Color32) {
    let bg = Color32::from_rgba_premultiplied(c.r(), c.g(), c.b(), 30);
    Frame::none().fill(bg).stroke(Stroke::new(1.0, Color32::from_rgba_premultiplied(c.r(), c.g(), c.b(), 80))).rounding(10.0).inner_margin(Margin { left: 8.0, right: 8.0, top: 2.0, bottom: 2.0 }).show(ui, |ui| {
        ui.label(RichText::new(text).size(11.0).color(c));
    });
    ui.add_space(4.0);
}

fn loading_center(ui: &mut Ui) {
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| { ui.spinner(); ui.add_space(6.0); ui.label(RichText::new("Loading…").color(c_text_faint())); });
    });
}

fn kv(ui: &mut Ui, label: &str, value: &str) {
    ui.label(RichText::new(label).size(12.0).color(c_text_dim()));
    ui.label(RichText::new(value).size(12.0).color(c_text()).monospace());
    ui.end_row();
}

// ── Button styles ─────────────────────────────────────────────────────────────

fn primary_btn(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).color(Color32::WHITE).size(13.0))
        .fill(c_accent()).rounding(Rounding::same(6.0)).stroke(Stroke::NONE))
}
fn accent_btn(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).color(Color32::WHITE).size(12.0))
        .fill(c_accent_dim()).rounding(Rounding::same(5.0)).stroke(Stroke::NONE))
}
fn danger_btn(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).color(Color32::WHITE).size(12.0))
        .fill(c_danger_dim()).rounding(Rounding::same(5.0)))
}
fn warn_btn(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).color(Color32::from_rgb(30, 20, 0)).size(12.0))
        .fill(Color32::from_rgb(180, 120, 0)).rounding(Rounding::same(5.0)).stroke(Stroke::NONE))
}
fn ghost_btn(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).color(c_text_dim()).size(12.0))
        .fill(Color32::TRANSPARENT).stroke(Stroke::new(1.0, c_border())).rounding(Rounding::same(5.0)))
}

// ── State color helpers ───────────────────────────────────────────────────────

fn job_color(state: &str) -> Color32 {
    match state {
        "done"       => c_success(),
        "running"    => c_accent(),
        "queued"     => c_warning(),
        "uploading"  => Color32::from_rgb(200, 150, 50),
        "failed"     => c_danger(),
        "cancelled"  => c_text_faint(),
        _            => c_text_dim(),
    }
}

fn job_icon(state: &str) -> String {
    let icon = match state {
        "done"      => "✓ done",
        "running"   => "▶ running",
        "queued"    => "◷ queued",
        "uploading" => "↑ uploading",
        "failed"    => "✗ failed",
        "cancelled" => "⊗ cancelled",
        _           => state,
    };
    icon.into()
}

// ─── Entry point ──────────────────────────────────────────────────────────────

fn main() -> eframe::Result<()> {
    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Tokio runtime"),
    );

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("AvaGen Admin")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "AvaGen Admin",
        options,
        Box::new(|cc| {
            setup_style(&cc.egui_ctx);
            Ok(Box::new(App::new(rt, cc)))
        }),
    )
}
