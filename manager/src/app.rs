use crate::launcher::Engine;
use crate::runtime::mem;
use crate::storage::db::*;
use crate::{runtime, ui, watcher};
use crossbeam_channel::{unbounded, Receiver, Sender};
use eframe::egui;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

#[derive(PartialEq, Clone, Copy)]
pub enum Nav {
    Accounts,
    Activity,
    Settings,
}

#[derive(PartialEq, Clone)]
pub enum GroupFilter {
    All,
    Ungrouped,
    Named(String),
}

/// Background threads call ping() -> UI wakes up and reloads from the DB. The UI never polls, so an
/// idle manager does no work at all.
#[derive(Clone)]
pub struct UiTx {
    pub tx: Sender<()>,
    pub ctx: egui::Context,
}
impl UiTx {
    pub fn ping(&self) {
        let _ = self.tx.send(());
        self.ctx.request_repaint();
    }
}

/// Text-field buffers for the account being edited (committed on focus loss / valid input).
#[derive(Default)]
pub struct EditBufs {
    pub for_id: Option<i64>,
    pub label: String,
    pub group: String,
    pub place_input: String,
    pub place_msg: Option<(bool, String)>,
    pub show_preset: bool,
}

pub struct App {
    pub engine: Engine,
    pub db: Db,
    pub rx: Receiver<()>,
    _rt: tokio::runtime::Runtime,
    pub nav: Nav,
    pub accounts: Vec<Account>,
    pub presets: Vec<Preset>,
    pub logs: Vec<Ev>,
    pub last_event: Option<Ev>,
    pub selected: Option<i64>,
    pub checked: BTreeSet<i64>,
    pub search: String,
    pub group_filter: GroupFilter,
    pub edit: EditBufs,
    pub confirm_remove: Option<i64>,
    pub confirm_clear: bool,
    pub paste_buf: String,
    pub sel_preset: i64,
    pub notice: Option<(bool, String)>,
    pub log_account: Option<i64>,
    pub log_problems_only: bool,
    pub mem: Option<mem::Mem>,
    mem_at: Instant,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        ui::theme::apply(&cc.egui_ctx);
        let db = Db::open().expect("cannot open database");
        db.reset_statuses();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("launch")
            .enable_all()
            .build()
            .expect("tokio runtime");
        let (tx, rx) = unbounded();
        let (wake_tx, wake_rx) = unbounded();
        let engine = Engine::new(db.clone(), rt.handle().clone(), UiTx { tx, ctx: cc.egui_ctx.clone() }, wake_tx);
        watcher::process::spawn(engine.clone(), wake_rx);
        runtime::trim::spawn(engine.clone());
        spawn_idle_memory_sample(engine.clone());

        let multi = engine.settings.read().unwrap().multi_instance;
        engine.set_multi_instance(multi);
        let mut app = App {
            engine,
            db,
            rx,
            _rt: rt,
            nav: Nav::Accounts,
            accounts: vec![],
            presets: vec![],
            logs: vec![],
            last_event: None,
            selected: None,
            checked: BTreeSet::new(),
            search: String::new(),
            group_filter: GroupFilter::All,
            edit: EditBufs::default(),
            confirm_remove: None,
            confirm_clear: false,
            paste_buf: String::new(),
            sel_preset: 1,
            notice: None,
            log_account: None,
            log_problems_only: false,
            mem: mem::current(),
            mem_at: Instant::now(),
        };
        app.reload();
        if let Some(p) = app.presets.first() {
            app.sel_preset = p.id;
        }
        app.selected = app.accounts.first().map(|a| a.id);
        app
    }

    pub fn reload(&mut self) {
        self.accounts = self.db.list_accounts().unwrap_or_default();
        self.presets = self.db.list_presets().unwrap_or_default();
        let ids: BTreeSet<i64> = self.accounts.iter().map(|a| a.id).collect();
        self.checked.retain(|id| ids.contains(id));
        if self.selected.map_or(true, |s| !ids.contains(&s)) {
            self.selected = self.accounts.first().map(|a| a.id);
        }
        let n = if self.nav == Nav::Activity { 500 } else { 1 };
        let ev = self.db.recent_events(n).unwrap_or_default();
        self.last_event = ev.last().cloned();
        if self.nav == Nav::Activity {
            self.logs = ev;
        }
    }
}

/// One-shot: 5 minutes after start, record this process's memory in the Activity log. If nothing was
/// launched in the meantime that's the "idle" figure; compare it with the 20 MB budget.
fn spawn_idle_memory_sample(engine: Engine) {
    std::thread::Builder::new()
        .name("mem-sample".into())
        .stack_size(128 * 1024)
        .spawn(move || {
            std::thread::sleep(Duration::from_secs(300));
            let Some(m) = mem::current() else { return };
            let idle = engine.tr().is_empty() && engine.db.own_launch_times().iter().all(|t| *t < now() - 300);
            engine.log(
                0,
                "diagnostic",
                &format!(
                    "manager memory after 5 min{}: working set {}, private {}, peak {}",
                    if idle { " idle" } else { " (clients were launched — not an idle figure)" },
                    mem::mb(m.working_set),
                    mem::mb(m.private),
                    mem::mb(m.peak_working_set)
                ),
            );
        })
        .ok();
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        let mut dirty = false;
        while self.rx.try_recv().is_ok() {
            dirty = true;
        }
        if dirty {
            self.reload();
        }
        // Only refreshed when we're repainting anyway (input or a ping) — never schedules a repaint.
        if self.mem_at.elapsed() > Duration::from_secs(2) {
            self.mem = mem::current();
            self.mem_at = Instant::now();
        }
        ui::draw(self, ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.engine.settings.read().unwrap().kill_on_exit {
            self.engine.kill_all_blocking();
        }
    }
}
