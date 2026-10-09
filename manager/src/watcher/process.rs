use super::crash_detect::{self, LogTail};
use super::{winscan, State};
use crate::launcher::Engine;
use crossbeam_channel::Receiver;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;
use sysinfo::{Pid, ProcessesToUpdate, System};

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
        let pids: Vec<Pid> = snap.iter().map(|x| Pid::from_u32(x.1)).collect();
        sys.refresh_processes(ProcessesToUpdate::Some(&pids), true);
        let tracked: Vec<u32> = snap.iter().map(|x| x.1).collect();
        let wins = winscan::scan(&tracked);
        let mut fails: Vec<(i64, u32, String, bool)> = vec![];

        for (id, pid, st) in snap {
            let proc_ = sys.process(Pid::from_u32(pid)).filter(|p| p.start_time() == st);
            if let Some(p) = proc_ {
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
            if !(s.log_tail || rejoin_on) {
                continue;
            }
            let mut job: Option<String> = None;
            let mut disc = false;
            {
                let mut t = self.tr();
                let claimed: HashSet<PathBuf> =
                    t.values().filter_map(|x| x.log.as_ref().map(|l| l.path.clone())).collect();
                if let Some(e) = t.get_mut(&id) {
                    // stable for 60s => forgive previous attempts
                    if e.attempts > 0 && e.live_since.map_or(false, |l| l.elapsed() >= Duration::from_secs(60)) {
                        e.attempts = 0;
                    }
                    let want_job = rejoin_on && !e.job_captured;
                    if e.log.is_none() && (want_job || s.log_tail) {
                        e.log = LogTail::claim(e.launch_ts, &claimed);
                    }
                    if want_job || s.log_tail {
                        if let Some(l) = e.log.as_mut() {
                            let txt = l.read_new();
                            if want_job {
                                if let Some(j) = crash_detect::find_job_id(&txt) {
                                    job = Some(j);
                                    e.job_captured = true;
                                }
                            }
                            if s.log_tail && crash_detect::has_disconnect(&txt) {
                                disc = true;
                            }
                        }
                    }
                }
            }
            if let Some(j) = job {
                let _ = self.db.set_job(id, Some(&j));
                self.ui.ping();
            }
            if disc {
                fails.push((id, pid, "disconnect found in Roblox log".into(), true));
            }
        }

        for (id, pid, reason, close) in fails {
            {
                let mut t = self.tr();
                match t.get_mut(&id) {
                    Some(e) if !e.user_killed => {
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
             (Settings → Run several accounts at once)"
                .into()
        } else {
            "client process exited".into()
        }
    }
}
