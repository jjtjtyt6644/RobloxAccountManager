pub mod fastflags;
pub mod spawn;
pub mod uri;
pub mod uri_probe;

use crate::app::UiTx;
use crate::auth::login::{self, LoginErr};
use crate::auth::ticket::{self, TicketErr};
use crate::runtime::singleton::{self, SingletonGuard};
use crate::runtime::priority;
use crate::storage::db::*;
use crate::watcher::{crash_detect, winscan, State, Tracked};
use crossbeam_channel::Sender;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::{ProcessesToUpdate, System};
use uri_probe::UriSchema;
use zeroize::Zeroizing;

/// Shared handle to everything background work needs. Cheap to clone (all Arcs).
///
/// Threads:
///   * UI thread (eframe)           — never blocks on network, process scans or window walks.
///   * tokio runtime, 2 workers     — launches (ticket fetch, sleeps). Anything blocking inside a
///                                    launch (ShellExecute, process scans, window scans, file writes)
///                                    goes through `spawn_blocking`.
///   * watcher thread               — 1 s health tick for all clients.
///   * short-lived std threads      — sign-in helper, stop/close sequences, log probe.
/// Locks: `track` (std Mutex) and the DB mutex are never held at the same time and never across an
/// `.await`. `gate` (tokio Mutex, FIFO) serialises the flag-write → client-ready window only.
#[derive(Clone)]
pub struct Engine {
    pub db: Db,
    pub rt: tokio::runtime::Handle,
    pub ui: UiTx,
    pub track: Arc<Mutex<HashMap<i64, Tracked>>>,
    pub settings: Arc<RwLock<Settings>>,
    pub http: reqwest::Client,
    pub gate: Arc<tokio::sync::Mutex<()>>,
    pub wake: Sender<()>,
    pub login_busy: Arc<AtomicBool>,
    pub gen: Arc<AtomicU64>,
    pub schema: Arc<RwLock<Option<UriSchema>>>,
    pub last_bind: Arc<Mutex<Option<(i64, Instant)>>>,
    pub probe_note: Arc<Mutex<String>>,
    /// Held while "Run several accounts at once" is on (runtime::singleton).
    pub singleton: Arc<Mutex<Option<SingletonGuard>>>,
}

fn roblox_pids() -> Vec<u32> {
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    sys.processes()
        .values()
        .filter(|p| p.name().to_string_lossy().eq_ignore_ascii_case("RobloxPlayerBeta.exe"))
        .map(|p| p.pid().as_u32())
        .collect()
}

fn find_new_client(since_secs: u64, taken: &HashSet<u32>) -> Option<(u32, u64)> {
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    sys.processes()
        .values()
        .filter(|p| {
            p.name().to_string_lossy().eq_ignore_ascii_case("RobloxPlayerBeta.exe")
                && p.start_time() + 2 >= since_secs
                && !taken.contains(&p.pid().as_u32())
        })
        .min_by_key(|p| p.start_time())
        .map(|p| (p.pid().as_u32(), p.start_time()))
}

impl Engine {
    pub fn new(db: Db, rt: tokio::runtime::Handle, ui: UiTx, wake: Sender<()>) -> Self {
        let settings = Arc::new(RwLock::new(db.load_settings()));
        let schema = db.get_kv("uri_schema").and_then(|s| serde_json::from_str::<UriSchema>(&s).ok());
        Engine {
            db,
            rt,
            ui,
            track: Default::default(),
            settings,
            http: reqwest::Client::builder().user_agent("Mozilla/5.0").build().expect("http client"),
            gate: Default::default(),
            wake,
            login_busy: Default::default(),
            gen: Default::default(),
            schema: Arc::new(RwLock::new(schema)),
            last_bind: Default::default(),
            probe_note: Default::default(),
            singleton: Default::default(),
        }
    }

    pub fn tr(&self) -> MutexGuard<'_, HashMap<i64, Tracked>> {
        self.track.lock().unwrap()
    }
    pub fn log(&self, id: i64, ev: &str, detail: &str) {
        let _ = self.db.log_event(id, ev, detail);
        self.ui.ping();
    }
    pub fn set_status(&self, id: i64, s: &str) {
        let _ = self.db.set_status(id, s);
        self.ui.ping();
    }
    pub fn clear(&self, id: i64, status: &str) {
        self.tr().remove(&id);
        self.set_status(id, status);
    }
    /// Is launch `gen` still the live intent for this account (not stopped, not superseded)?
    fn current(&self, id: i64, gen: u64) -> bool {
        self.tr().get(&id).map_or(false, |e| e.gen == gen && !e.user_killed)
    }
    // ---------- several clients at once ----------
    pub fn set_multi_instance(&self, on: bool) {
        *self.singleton.lock().unwrap() = on.then(singleton::acquire);
        self.ui.ping();
    }

    /// Some(true) = active, Some(false) = blocked by a Roblox client that started first, None = off.
    pub fn multi_instance_state(&self) -> Option<bool> {
        self.singleton.lock().unwrap().as_ref().map(|g| !g.blocked)
    }

    /// Can a new client start without closing one that's already open? Retries taking the lock if a
    /// Roblox client had it before (it may have been closed since). Always true when the setting is off.
    fn multi_instance_ok(&self) -> bool {
        if !self.settings.read().unwrap().multi_instance {
            return true;
        }
        let mut g = self.singleton.lock().unwrap();
        if g.as_ref().map_or(true, |g| g.blocked) {
            *g = Some(singleton::acquire());
        }
        g.as_ref().map_or(false, |g| !g.blocked)
    }

    /// Settings → "Close all Roblox windows": closes every RobloxPlayerBeta.exe (ours or not), waits for
    /// them to go, then takes the single-instance lock so the next launches can run side by side.
    pub fn close_all_roblox_and_fix(&self) {
        let e = self.clone();
        std::thread::Builder::new()
            .name("close-all-roblox".into())
            .spawn(move || {
                e.kill_all_blocking();
                let ids: Vec<i64> = e.tr().keys().copied().collect();
                for id in ids {
                    e.clear(id, "idle");
                }
                let mut sys = System::new();
                sys.refresh_processes(ProcessesToUpdate::All, true);
                for p in sys.processes().values() {
                    if p.name().to_string_lossy().eq_ignore_ascii_case("RobloxPlayerBeta.exe") {
                        p.kill();
                    }
                }
                let deadline = Instant::now() + Duration::from_secs(10);
                while !roblox_pids().is_empty() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(300));
                }
                e.set_multi_instance(true);
                let ok = e.multi_instance_state() == Some(true);
                e.log(
                    0,
                    "multi_instance",
                    if ok {
                        "all Roblox windows closed — several accounts can now run at once"
                    } else {
                        "a Roblox process is still running; close it in Task Manager, then try again"
                    },
                );
            })
            .ok();
    }

    pub fn rejoin_available(&self) -> bool {
        self.schema.read().unwrap().is_some()
    }

    // ---------- add account ----------
    pub fn start_login(&self) {
        if self.login_busy.swap(true, Ordering::SeqCst) {
            return;
        }
        self.ui.ping();
        let e = self.clone();
        std::thread::Builder::new()
            .name("sign-in".into())
            .spawn(move || match login::run() {
                Ok(cookie) => e.add_cookie(cookie),
                Err(er) => {
                    let (ev, msg) = match er {
                        LoginErr::Cancelled => ("login_cancelled", "sign-in window closed before signing in".to_string()),
                        LoginErr::HelperMissing(p) => {
                            ("add_failed", format!("{} is missing — it must sit next to the manager exe", p.display()))
                        }
                        LoginErr::Failed(m) => ("add_failed", m),
                    };
                    e.log(0, ev, &msg);
                    e.login_busy.store(false, Ordering::SeqCst);
                    e.ui.ping();
                }
            })
            .ok();
    }

    pub fn add_cookie(&self, cookie: Zeroizing<String>) {
        let e = self.clone();
        self.login_busy.store(true, Ordering::SeqCst);
        self.rt.spawn(async move {
            match ticket::whoami(&e.http, &cookie).await {
                Ok(name) => match e.db.upsert_account(&name, &cookie) {
                    Ok(id) => e.log(id, "account_added", &name),
                    Err(er) => e.log(0, "add_failed", &er.to_string()),
                },
                Err(er) => e.log(0, "add_failed", &er),
            }
            e.login_busy.store(false, Ordering::SeqCst);
            e.ui.ping();
        });
    }

    // ---------- same-server rejoin verification ----------
    pub fn probe_rejoin(&self) {
        *self.probe_note.lock().unwrap() = "Checking Roblox logs…".into();
        self.ui.ping();
        let e = self.clone();
        std::thread::Builder::new()
            .name("uri-probe".into())
            .spawn(move || {
                let own = e.db.own_launch_times();
                let rep = uri_probe::probe(&own);
                if let Some(s) = &rep.schema {
                    if let Ok(j) = serde_json::to_string(s) {
                        let _ = e.db.set_kv("uri_schema", &j);
                    }
                    *e.schema.write().unwrap() = Some(s.clone());
                }
                let msg = rep.summary();
                *e.probe_note.lock().unwrap() = msg.clone();
                e.log(0, "probe", &msg);
            })
            .ok();
    }

    pub fn forget_schema(&self) {
        let _ = self.db.del_kv("uri_schema");
        *self.schema.write().unwrap() = None;
        let s = {
            let mut s = self.settings.write().unwrap();
            s.same_server_rejoin = false;
            s.clone()
        };
        let _ = self.db.save_settings(&s);
        *self.probe_note.lock().unwrap() = String::new();
        self.ui.ping();
    }

    // ---------- launch ----------
    pub fn launch(&self, id: i64) {
        let e = self.clone();
        self.rt.spawn(async move { e.do_launch(id).await });
    }

    pub fn launch_many(&self, ids: Vec<i64>) {
        for id in ids {
            self.launch(id);
        }
    }

    pub async fn do_launch(&self, id: i64) {
        let Ok(acc) = self.db.get_account(id) else { return };
        if acc.place_id.is_empty() || !acc.place_id.bytes().all(|b| b.is_ascii_digit()) {
            self.log(id, "blocked", "no game set — add a Place ID or game link first");
            return;
        }
        let gen = self.gen.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut t = self.tr();
            let e = t.entry(id).or_default();
            if matches!(e.state, State::Starting | State::Live) {
                return;
            }
            e.gen = gen;
            e.state = State::Starting;
            e.pid = None;
            e.user_killed = false;
            e.log = None;
            e.job_captured = false;
        }
        self.set_status(id, "queued");

        // One client at a time from flag-write until the client has its window up.
        let _g = self.gate.lock().await;
        if !self.current(id, gen) {
            return; // stopped while queued
        }
        if !self.multi_instance_ok() {
            self.log(
                id,
                "blocked",
                "a Roblox window was already open before \"Run several accounts at once\" could switch on, so                  opening another would close it. Close every Roblox window (Settings has a button), then press Play.",
            );
            self.clear(id, "idle");
            return;
        }
        self.set_status(id, "launching");

        let preset = self.db.get_preset(acc.preset_id).unwrap_or_default();
        let gfx = preset.gfx.clone();
        let flags = tokio::task::spawn_blocking(move || fastflags::apply(&gfx))
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        if let Err(er) = flags {
            self.log(id, "flags_error", &er);
            self.clear(id, "idle");
            return;
        }

        let cookie = match self.db.get_cookie(id) {
            Ok(c) => c,
            Err(_) => {
                self.log(id, "cookie_error", "no usable cookie stored");
                self.clear(id, "needs_relogin");
                return;
            }
        };
        let mut n = 0u32;
        let ticket: Zeroizing<String> = loop {
            match ticket::fetch(&self.http, &cookie, &acc.place_id).await {
                Ok(t) => break Zeroizing::new(t),
                Err(TicketErr::Unauthorized) => {
                    self.log(id, "auth_expired", "cookie rejected (401)");
                    self.clear(id, "needs_relogin");
                    return;
                }
                Err(TicketErr::RateLimited) if n < 4 => {
                    n += 1;
                    tokio::time::sleep(Duration::from_secs(2u64.pow(n))).await;
                }
                Err(er) => {
                    let retry = self.settings.read().unwrap().retry_on_auth_fail;
                    self.log(id, "ticket_error", &format!("{er:?}"));
                    if retry {
                        self.on_failure(id, "ticket fetch failed".into());
                    } else {
                        self.clear(id, "crashed");
                    }
                    return;
                }
            }
        };
        drop(cookie); // zeroized on drop
        if !self.current(id, gen) {
            return;
        }

        // Same-server rejoin only when: verified from a captured real URI, enabled by the user, this is
        // a reconnect (not a fresh manual launch), and fewer than 3 tries (the server may be gone).
        let s = self.settings.read().unwrap().clone();
        let attempts = self.tr().get(&id).map(|e| e.attempts).unwrap_or(0);
        let schema = self.schema.read().unwrap().clone();
        let rejoin: Option<(UriSchema, String)> = match (schema, acc.last_job_id.clone()) {
            (Some(sc), Some(j)) if s.same_server_rejoin && (1..=2).contains(&attempts) => Some((sc, j)),
            _ => None,
        };

        let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        let link: Zeroizing<String> = Zeroizing::new(match &rejoin {
            Some((sc, j)) => uri_probe::build_rejoin(sc, &ticket, &acc.place_id, j),
            None => uri::build(&ticket, ts, &acc.place_id),
        });
        drop(ticket);
        let opened = tokio::task::spawn_blocking(move || spawn::open(&link))
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        if let Err(er) = opened {
            self.log(id, "spawn_error", &er);
            self.on_failure(id, "could not start Roblox".into());
            return;
        }
        self.log(
            id,
            "launch",
            &format!(
                "place {}{}",
                acc.place_id,
                if rejoin.is_some() { " — trying previous server (best-effort)" } else { "" }
            ),
        );

        // Bind PID: look for a new RobloxPlayerBeta.exe every 1.5 s, 30 s timeout. Keep looking even if
        // the user pressed Stop meanwhile, so we can close the client instead of orphaning it.
        let deadline = Instant::now() + Duration::from_secs(30);
        let bound = loop {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let taken: HashSet<u32> = self.tr().values().filter_map(|t| t.pid).collect();
            let found = tokio::task::spawn_blocking(move || find_new_client(ts, &taken)).await.ok().flatten();
            if found.is_some() || Instant::now() > deadline {
                break found;
            }
        };

        let Some((pid, st)) = bound else {
            if self.current(id, gen) {
                self.on_failure(id, "launch timeout (no Roblox client appeared within 30 s)".into());
            }
            return;
        };
        if !self.current(id, gen) {
            let _ = tokio::task::spawn_blocking(move || crash_detect::graceful_close(pid)).await;
            return;
        }
        {
            let mut t = self.tr();
            let e = t.entry(id).or_default();
            e.pid = Some(pid);
            e.start_time = st;
            e.launch_ts = ts;
            e.state = State::Live;
            e.live_since = Some(Instant::now());
            e.live_since_ts = now();
        }
        *self.last_bind.lock().unwrap() = Some((id, Instant::now()));
        priority::apply(pid, preset.priority);
        self.db.touch_used(id);
        self.set_status(id, "live");
        let _ = self.wake.send(());

        // Keep the gate until this client shows its main window: by then it has read
        // ClientAppSettings.json, so the next launch can't overwrite flags it hasn't loaded yet
        // (matters when accounts use different presets).
        let ready_by = Instant::now() + Duration::from_secs(20);
        loop {
            let ready = tokio::task::spawn_blocking(move || winscan::has_main_window(&winscan::scan(&[pid]), pid))
                .await
                .unwrap_or(true);
            if ready || Instant::now() > ready_by {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        tokio::time::sleep(Duration::from_millis(1000)).await; // small spacing between clients
    }
}
