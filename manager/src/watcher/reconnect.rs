use super::crash_detect;
use super::State;
use crate::launcher::Engine;
use std::time::Duration;
use sysinfo::{ProcessesToUpdate, System};

impl Engine {
    /// Retry state machine: attempts++ -> over max => crashed, else wait delay and re-run full launch.
    /// Whether the relaunch targets the previous server is decided in do_launch (only when same-server
    /// rejoin is verified AND enabled; otherwise it's a plain relaunch).
    pub fn on_failure(&self, id: i64, reason: String) {
        let s = self.settings.read().unwrap().clone();
        let attempts = {
            let mut t = self.tr();
            let Some(e) = t.get_mut(&id) else { return }; // removed (user stopped it) — nothing to recover
            if e.user_killed {
                return;
            }
            e.pid = None;
            e.state = State::Reconnecting;
            e.attempts += 1;
            e.attempts
        };
        self.log(id, "failure", &format!("{reason} (attempt {attempts}/{})", s.max_attempts));
        if !s.auto_reconnect || attempts > s.max_attempts {
            self.clear(id, "crashed");
            return;
        }
        self.set_status(id, "reconnecting");
        let e = self.clone();
        self.rt.spawn(async move {
            tokio::time::sleep(Duration::from_secs(s.delay_secs)).await;
            let cancelled = e.tr().get(&id).map(|t| t.user_killed).unwrap_or(true);
            if !cancelled {
                e.do_launch(id).await;
            }
        });
    }

    /// User-initiated stop. Never triggers a reconnect. Safe at any stage: queued, starting
    /// (the launch task notices via its generation check and closes the client itself), live,
    /// or waiting to reconnect.
    pub fn kill(&self, id: i64) {
        let pid = {
            let mut t = self.tr();
            match t.get_mut(&id) {
                Some(e) => {
                    e.user_killed = true;
                    e.pid
                }
                None => None,
            }
        };
        let e = self.clone();
        std::thread::Builder::new()
            .name(format!("stop-{id}"))
            .spawn(move || {
                if let Some(pid) = pid {
                    crash_detect::graceful_close(pid);
                }
                e.clear(id, "idle");
                e.log(id, "kill", "stopped by user");
            })
            .ok();
    }

    pub fn kill_many(&self, ids: Vec<i64>) {
        for id in ids {
            self.kill(id);
        }
    }

    pub fn remove_account(&self, id: i64) {
        let running = self.tr().contains_key(&id);
        if running {
            self.kill(id);
        }
        let e = self.clone();
        std::thread::spawn(move || {
            if running {
                std::thread::sleep(Duration::from_secs(5)); // let the stop finish first
            }
            let _ = e.db.delete_account(id);
            e.ui.ping();
        });
    }

    pub fn kill_all_blocking(&self) {
        let pids: Vec<sysinfo::Pid> = {
            let mut t = self.tr();
            t.values_mut().for_each(|e| e.user_killed = true);
            t.values().filter_map(|t| t.pid).map(sysinfo::Pid::from_u32).collect()
        };
        let mut s = System::new();
        s.refresh_processes(ProcessesToUpdate::Some(&pids), true);
        for p in pids {
            if let Some(pr) = s.process(p) {
                pr.kill();
            }
        }
    }
}
