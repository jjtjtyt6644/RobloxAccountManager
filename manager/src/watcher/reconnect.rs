use super::crash_detect;
use super::State;
use crate::launcher::Engine;
use std::time::Duration;

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
        self.set_status(id, "stopping");
        let e = self.clone();
        std::thread::Builder::new()
            .name(format!("stop-{id}"))
            .spawn(move || {
                let ended = pid.map_or(true, crash_detect::graceful_close);
                e.clear(id, "idle");
                e.after_client_exit(id);
                if ended {
                    e.log(id, "kill", "stopped by user — Roblox and its helper processes have ended");
                } else {
                    e.log(id, "kill_failed", "Windows wouldn't end this Roblox window; close it from Task Manager");
                }
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
        let pids: Vec<u32> = {
            let mut t = self.tr();
            t.values_mut().for_each(|e| e.user_killed = true);
            t.values().filter_map(|t| t.pid).collect()
        };
        // In parallel: closing the app shouldn't take 4 s per window.
        let hs: Vec<_> = pids.into_iter().map(|p| std::thread::spawn(move || crash_detect::graceful_close(p))).collect();
        for h in hs {
            let _ = h.join();
        }
    }
}
