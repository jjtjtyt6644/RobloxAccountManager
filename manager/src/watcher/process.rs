use super::crash_detect::{self, LogEvent, LogTail};
use super::{winscan, State};
use crate::launcher::Engine;
use crossbeam_channel::Receiver;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// After a disconnect line, wait this long for a rejoin (teleport / Roblox's Reconnect) before acting.
const REJOIN_GRACE: Duration = Duration::from_secs(10);
/// A freshly launched client that hasn't joined a game by then is stuck (error prompt, full server…).
const JOIN_TIMEOUT: Duration = Duration::from_secs(180);
/// Main window "not responding" this long = frozen.
const HUNG_FOR: Duration = Duration::from_secs(60);
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// One watcher thread for all clients. Blocks on a channel when nothing is tracked (0% CPU),
/// 1 s tick otherwise. Per-client work is small (one process refresh + one window scan for all
/// clients together), so a single thread scales fine to dozens of clients; recovery work
/// (closing windows, relaunching) is pushed onto other threads so a slow close never delays
/// detection for the other clients.
pub fn spawn(engine: Engine, rx: Receiver<()>) {
    std::thread::Builder::new()
        .name("watcher".into())
        .stack_size(512 * 1024)
        .spawn(move || {
            let mut sys = System::new();
            loop {
                let active = !engine.tr().is_empty();
                if active {
                    let _ = rx.recv_timeout(Duration::from_secs(1));
                } else if rx.recv().is_err() {
                    return;
                }
                while rx.try_recv().is_ok() {}
                engine.tick(&mut sys);
            }
        })
        .expect("spawn watcher thread");
}

static TICKS: AtomicU64 = AtomicU64::new(0);

impl Engine {
    fn tick(&self, sys: &mut System) {
        let s = self.settings.read().unwrap().clone();
        let rejoin_on = s.same_server_rejoin && self.schema.read().unwrap().is_some();
        let snap: Vec<(i64, u32, u64)> = self
            .tr()
            .iter()
            .filter(|(_, t)| t.state == State::Live && !t.user_killed)
            .filter_map(|(id, t)| t.pid.map(|p| (*id, p, t.start_time)))
            .collect();
        if snap.is_empty() {
            return;
        }
        // Every tick (1 s): is it alive, and new log lines (disconnects need to be noticed quickly).
        // Every 2nd tick: window checks (error dialogs, frozen, titles). Every 5th: memory figures.
        let n = TICKS.fetch_add(1, Ordering::Relaxed);
        let (scan_windows, with_mem) = (n % 2 == 0, n % 5 == 0);
        let pids: Vec<Pid> = snap.iter().map(|x| Pid::from_u32(x.1)).collect();
        let kind = if with_mem { ProcessRefreshKind::new().with_memory() } else { ProcessRefreshKind::new() };
        sys.refresh_processes_specifics(ProcessesToUpdate::Some(&pids), true, kind);
        let tracked: Vec<u32> = snap.iter().map(|x| x.1).collect();
        let wins = if scan_windows { winscan::scan(&tracked) } else { Vec::new() };
        let mut fails: Vec<(i64, u32, String, bool)> = vec![];

        for (id, pid, st) in snap {
            let proc_ = sys.process(Pid::from_u32(pid)).filter(|p| p.start_time() == st);
            if let Some(p) = proc_.filter(|_| with_mem) {
                if let Some(e) = self.tr().get_mut(&id) {
                    e.mem_ws = p.memory();
                }
            }
            if proc_.is_none() {
                fails.push((id, pid, self.exit_reason(id), false));
                continue;
            }
            if let Some(what) = winscan::error_summary(&wins, pid) {
                fails.push((id, pid, format!("error window detected — {what}"), true));
                continue;
            }
            // Frozen: the main window has been "not responding" for a while.
            if s.detect_disconnects && scan_windows {
                let hung = winscan::main_window_hung(&wins, pid);
                let mut t = self.tr();
                if let Some(e) = t.get_mut(&id) {
                    match (hung, e.hung_since) {
                        (true, None) => e.hung_since = Some(Instant::now()),
                        (true, Some(since)) if since.elapsed() >= HUNG_FOR => {
                            drop(t);
                            fails.push((id, pid, format!("stopped responding for {} s", HUNG_FOR.as_secs()), true));
                            continue;
                        }
                        (false, Some(_)) => e.hung_since = None,
                        _ => {}
                    }
                }
            }
            // Name the game window after its account so windows can be told apart on the taskbar.
            if let Some((h, text)) = winscan::main_window(&wins, pid) {
                let title = self.tr().get(&id).map(|t| t.title.clone()).unwrap_or_default();
                let want = if s.title_windows && !title.is_empty() {
                    format!("{}{title}", winscan::TITLE_PREFIX)
                } else {
                    "Roblox".to_string()
                };
                if text != want {
                    winscan::set_title(h, &want);
                }
            }
            if !(s.detect_disconnects || rejoin_on) {
                continue;
            }
            let mut job: Option<String> = None;
            let mut fail: Option<String> = None;
            let mut new_game: Option<u64> = None;
            let mut denied: Vec<String> = vec![];
            {
                let mut t = self.tr();
                let claimed: HashSet<PathBuf> =
                    t.values().filter_map(|x| x.log.as_ref().map(|l| l.path.clone())).collect();
                if let Some(e) = t.get_mut(&id) {
                    // stable for 60s => forgive previous attempts
                    if e.attempts > 0 && e.live_since.map_or(false, |l| l.elapsed() >= Duration::from_secs(60)) {
                        e.attempts = 0;
                    }
                    if e.log.is_none() {
                        e.log = LogTail::for_process(e.start_time, &claimed).or_else(|| LogTail::claim(e.launch_ts, &claimed));
                    }
                    let want_job = rejoin_on && !e.job_captured;
                    if let Some(l) = e.log.as_mut() {
                        let txt = l.read_new();
                        if want_job {
                            if let Some(j) = crash_detect::find_job_id(&txt) {
                                job = Some(j);
                                e.job_captured = true;
                            }
                        }
                        if let Some(p) = crash_detect::find_place(&txt) {
                            e.last_place = Some(p);
                        }
                        if !e.denied_reported {
                            let d = crash_detect::find_denied(&txt);
                            if !d.is_empty() {
                                e.denied_reported = true;
                                denied = d;
                            }
                        }
                        if let Some(u) = crash_detect::find_universe(&txt) {
                            if e.universe != Some(u) {
                                e.universe = Some(u);
                                new_game = Some(u);
                            }
                        }
                        if s.detect_disconnects {
                            // Replay in order: a later join cancels an earlier disconnect (teleports,
                            // Roblox's own Reconnect button).
                            for ev in crash_detect::scan_events(&txt) {
                                match ev {
                                    LogEvent::Joined => {
                                        e.joined = true;
                                        e.pending_fail = None;
                                    }
                                    LogEvent::Disconnected(r) if e.pending_fail.is_none() => {
                                        e.pending_fail = Some((Instant::now(), format!("disconnected from the game — {r}")));
                                    }
                                    LogEvent::LeftToHome if s.reopen_if_left && !e.home_launch && e.pending_fail.is_none() => {
                                        e.pending_fail =
                                            Some((Instant::now(), "left the game — back on the Roblox home screen".into()));
                                    }
                                    _ => {}
                                }
                            }
                            if let Some((at, r)) = &e.pending_fail {
                                if at.elapsed() >= REJOIN_GRACE {
                                    fail = Some(r.clone());
                                }
                            } else if !e.joined && !e.home_launch && e.live_since.map_or(false, |l| l.elapsed() >= JOIN_TIMEOUT) {
                                fail = Some(format!("didn't get into the game within {} minutes", JOIN_TIMEOUT.as_secs() / 60));
                            }
                        }
                    }
                }
            }
            if let Some(j) = job {
                let _ = self.db.set_job(id, Some(&j));
                self.ui.ping();
            }
            if !denied.is_empty() {
                self.log(id, "denied", &format!("Roblox ignored these profile settings: {}", denied.join(", ")));
            }
            if let Some(u) = new_game {
                self.game_name(u); // starts the lookup; the UI shows it when it arrives
                self.ui.repaint();
            }
            if let Some(r) = fail {
                fails.push((id, pid, r, true));
            }
        }

        for (id, pid, reason, close) in fails {
            {
                let mut t = self.tr();
                match t.get_mut(&id) {
                    Some(e) if !e.user_killed && e.pid == Some(pid) => {
                        e.state = State::Reconnecting; // stop the next tick re-reporting it
                        e.pid = None;
                        e.log = None;
                    }
                    _ => continue,
                }
            }
            let e = self.clone();
            std::thread::Builder::new()
                .name(format!("recover-{id}"))
                .spawn(move || {
                    if close {
                        crash_detect::graceful_close(pid);
                    }
                    e.after_client_exit(id);
                    e.on_failure(id, reason);
                })
                .ok();
        }
    }

    /// If a client vanished right after ANOTHER account's client started, the usual cause is Roblox's
    /// single-instance check closing the older window. Say so instead of a bare "exited".
    fn exit_reason(&self, id: i64) -> String {
        let last: Option<(i64, std::time::Instant)> = *self.last_bind.lock().unwrap();
        let recent_other = matches!(last, Some((other, at)) if other != id && at.elapsed() < Duration::from_secs(15));
        if recent_other {
            "client closed right after another account's client started — Roblox's one-window limit \
             (Settings > Run several accounts at once)"
                .into()
        } else {
            "client process exited".into()
        }
    }
}
