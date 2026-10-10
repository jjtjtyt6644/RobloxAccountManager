//! Finds Roblox clients the manager didn't start — opened from the website, by another tool, or
//! before the manager was opened — and works out which account each one is.
//!
//! How: every few seconds, list RobloxPlayerBeta.exe processes nobody is tracking. Each client writes
//! its own log in %LOCALAPPDATA%\Roblox\logs whose file name carries the time it started
//! (…_20261009T080341Z_Player_…), so a process is matched to the log created within a few seconds of
//! the process's start time. Once the client joins a game its log contains
//!   [FLog::GameJoinLoadTime] Report game_join_loadtime: placeid:…, …, userid:11780444860,
//! and that user id is looked up among the accounts (accounts.user_id). A match is tracked exactly
//! like a client the manager launched itself: watched, reconnected, memory/CPU saver, Stop.
//! Clients of accounts that aren't in the manager (or still on the loading screen) are listed as
//! "other Roblox windows" — the memory and CPU savers still apply to them.
//!
//! Cost: one process-name scan (no CPU/memory/disk queries) every 6 s, plus reading new log lines of
//! the unidentified clients only. Logs are read incrementally, never whole files twice.
use super::crash_detect::{log_time, logs_dir, LogTail};
use super::State;
use crate::launcher::Engine;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

/// A Roblox client that isn't one the manager is tracking for an account.
#[derive(Clone, PartialEq)]
pub struct OtherClient {
    pub pid: u32,
    /// Known once the client has joined a game.
    pub user_id: Option<u64>,
}

/// Last "userid:<digits>" in `text`.
pub fn find_user_id(text: &str) -> Option<u64> {
    const KEY: &str = "userid:";
    let i = text.rfind(KEY)? + KEY.len();
    let d: String = text[i..].chars().take_while(|c| c.is_ascii_digit()).collect();
    d.parse().ok().filter(|v| *v > 0)
}

/// Per unidentified client: its log (read incrementally) and what we've learnt.
struct Seen {
    start: u64,
    log: Option<LogTail>,
    user_id: Option<u64>,
    /// Game it's in (universe id), from the same log.
    universe: Option<u64>,
}

impl Engine {
    /// One detection pass. `seen` persists between passes. Returns true while a new window hasn't been
    /// identified yet (the caller then checks again within a fraction of a second, so a second window
    /// of an open account is caught right as it joins, before it can get the first one kicked).
    fn detect_pass(&self, sys: &mut System, seen: &mut HashMap<u32, Seen>) -> bool {
        // A launch in progress may be about to bind a brand-new client; let it, then look again.
        let busy = self.tr().values().any(|t| matches!(t.state, State::Starting | State::Reconnecting));
        if busy {
            return false;
        }
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::new());
        let tracked: HashSet<u32> = self.tr().values().filter_map(|t| t.pid).collect();
        let roblox: Vec<(u32, u64)> = sys
            .processes()
            .values()
            .filter(|p| p.name().to_string_lossy().eq_ignore_ascii_case("RobloxPlayerBeta.exe"))
            .map(|p| (p.pid().as_u32(), p.start_time()))
            .filter(|(pid, _)| !tracked.contains(pid))
            .collect();
        let live: HashSet<u32> = roblox.iter().map(|r| r.0).collect();
        seen.retain(|pid, s| live.contains(pid) && roblox.iter().any(|r| r.0 == *pid && r.1 == s.start));

        if !roblox.is_empty() {
            // Logs already in use: by tracked clients and by clients matched in earlier passes.
            let mut claimed: HashSet<PathBuf> = self.tr().values().filter_map(|t| t.log.as_ref().map(|l| l.path.clone())).collect();
            claimed.extend(seen.values().filter_map(|s| s.log.as_ref().map(|l| l.path.clone())));
            let logs: Vec<(u64, PathBuf)> = logs_dir()
                .and_then(|d| std::fs::read_dir(d).ok())
                .map(|rd| {
                    rd.flatten()
                        .filter_map(|e| {
                            let name = e.file_name().to_string_lossy().into_owned();
                            (name.ends_with(".log") && name.contains("_Player_") && !name.contains("CrashHandler"))
                                .then(|| log_time(&name).map(|t| (t, e.path())))
                                .flatten()
                        })
                        .collect()
                })
                .unwrap_or_default();

            let mut order = roblox.clone();
            order.sort_by_key(|r| r.1);
            for (pid, st) in order {
                let s = seen.entry(pid).or_insert(Seen { start: st, log: None, user_id: None, universe: None });
                if s.log.is_none() {
                    // The log created closest to this process's start (it's written within ~1 s).
                    let best = logs
                        .iter()
                        .filter(|(t, p)| !claimed.contains(p) && *t + 3 >= st && *t <= st + 20)
                        .min_by_key(|(t, _)| t.abs_diff(st));
                    if let Some((_, p)) = best {
                        claimed.insert(p.clone());
                        s.log = Some(LogTail::at(p.clone(), 0));
                    }
                }
                if s.user_id.is_none() {
                    if let Some(l) = s.log.as_mut() {
                        let txt = l.read_new();
                        s.user_id = find_user_id(&txt);
                        s.universe = super::crash_detect::find_universe(&txt).or(s.universe);
                    }
                }
            }
        }

        // One window per account: close the newer of two windows signed into the same account.
        if self.settings.read().unwrap().one_window_per_account {
            self.close_duplicates(seen);
        }

        // Identified clients of our accounts become tracked.
        let presets = self.db.list_presets().unwrap_or_default();
        let mut adopted = vec![];
        for (pid, s) in seen.iter_mut() {
            let Some(uid) = s.user_id else { continue };
            let Some(id) = self.db.account_by_user_id(uid) else { continue };
            if self.tr().contains_key(&id) {
                continue; // that account already has a client (two windows of one account: leave it listed)
            }
            let Ok(acc) = self.db.get_account(id) else { continue };
            let prio = presets.iter().find(|p| p.id == acc.preset_id).map_or(1, |p| p.priority);
            let log = s.log.take().map(|l| {
                let end = std::fs::metadata(&l.path).map(|m| m.len()).unwrap_or(0);
                LogTail::at(l.path, end)
            });
            self.track_existing(id, *pid, s.start, s.start, s.start as i64, prio, log);
            if let Some(e) = self.tr().get_mut(&id) {
                e.title = acc.label.clone();
                e.universe = s.universe;
            }
            self.log(id, "detected", &format!("found it already running (PID {pid}), started outside the manager — now watching it"));
            adopted.push(*pid);
        }
        for p in &adopted {
            seen.remove(p);
        }
        if !adopted.is_empty() {
            let _ = self.wake.send(());
        }

        let others: Vec<OtherClient> = {
            let mut v: Vec<OtherClient> = seen.iter().map(|(pid, s)| OtherClient { pid: *pid, user_id: s.user_id }).collect();
            v.sort_by_key(|o| o.pid);
            v
        };
        let mut cur = self.others.lock().unwrap();
        if *cur != others || !adopted.is_empty() {
            *cur = others;
            drop(cur);
            self.ui.ping();
        }
        let now = crate::storage::db::now() as u64;
        seen.values().any(|s| s.user_id.is_none() && s.log.is_some() && now.saturating_sub(s.start) < FAST_FOR)
    }

    /// For every account with two windows, end the NEWER one and keep the one that was there first.
    /// Covers a tracked window + a newly opened one (either may be the newer), and two windows the
    /// manager isn't tracking.
    fn close_duplicates(&self, seen: &mut HashMap<u32, Seen>) {
        // user id -> (start time, pid, Some(account id) if tracked)
        let mut first: HashMap<u64, (u64, u32, Option<i64>)> = HashMap::new();
        let tracked: Vec<(i64, u32, u64)> =
            self.tr().iter().filter_map(|(id, t)| t.pid.filter(|_| t.state == State::Live).map(|p| (*id, p, t.start_time))).collect();
        for (id, pid, st) in tracked {
            if let Some(uid) = self.db.user_id_of(id) {
                first.insert(uid, (st, pid, Some(id)));
            }
        }
        let mut ids: Vec<(u32, u64, u64)> = seen.iter().filter_map(|(p, s)| s.user_id.map(|u| (*p, u, s.start))).collect();
        ids.sort_by_key(|x| x.2);
        for (pid, uid, st) in ids {
            let Some(&(kept_st, kept_pid, kept_acc)) = first.get(&uid) else {
                first.insert(uid, (st, pid, None));
                continue;
            };
            let account = self.db.account_by_user_id(uid).unwrap_or(0);
            if st >= kept_st {
                // The usual case: a new window for an account that's already open. End it now.
                crate::watcher::crash_detect::end_now(pid);
                seen.remove(&pid);
                self.log(
                    account,
                    "duplicate",
                    &format!("closed a second Roblox window for this account (PID {pid}) — kept the one that was already open (PID {kept_pid})"),
                );
            } else if let Some(acc_id) = kept_acc {
                // The tracked window is the newer one: switch tracking to the older window, then end the newer.
                let s = seen.remove(&pid).unwrap();
                let prio = self.tr().get(&acc_id).map_or(1, |t| t.base_priority);
                let title = self.tr().get(&acc_id).map(|t| t.title.clone()).unwrap_or_default();
                let log = s.log.map(|l| {
                    let end = std::fs::metadata(&l.path).map(|m| m.len()).unwrap_or(0);
                    LogTail::at(l.path, end)
                });
                self.track_existing(acc_id, pid, s.start, s.start, s.start as i64, prio, log);
                if let Some(e) = self.tr().get_mut(&acc_id) {
                    e.title = title;
                    e.universe = s.universe;
                }
                crate::watcher::crash_detect::end_now(kept_pid);
                self.log(
                    acc_id,
                    "duplicate",
                    &format!("closed a second Roblox window for this account (PID {kept_pid}) — kept the one that was open first (PID {pid})"),
                );
                first.insert(uid, (st, pid, Some(acc_id)));
            } else {
                // Two untracked windows; this one is older: end the other.
                crate::watcher::crash_detect::end_now(kept_pid);
                seen.remove(&kept_pid);
                self.log(
                    account,
                    "duplicate",
                    &format!("closed a second Roblox window for the same account (PID {kept_pid}) — kept the one that was open first (PID {pid})"),
                );
                first.insert(uid, (st, pid, None));
            }
            self.ui.ping();
        }
    }
}

/// How long after a window starts the detector keeps checking it several times a second.
const FAST_FOR: u64 = 90;

/// Fills in Roblox user ids for accounts added before 1.3.0 (public lookup by username).
pub fn backfill_user_ids(engine: &Engine) {
    let missing = engine.db.missing_user_ids();
    if missing.is_empty() {
        return;
    }
    let e = engine.clone();
    engine.rt.spawn(async move {
        let names: Vec<String> = missing.iter().map(|m| m.1.clone()).collect();
        if let Ok(found) = crate::auth::ticket::user_ids(&e.http, &names).await {
            for (id, name) in missing {
                if let Some((_, uid)) = found.iter().find(|(n, _)| n.eq_ignore_ascii_case(&name)) {
                    e.db.set_user_id(id, *uid);
                }
            }
        }
    });
}

pub fn spawn(engine: Engine) {
    backfill_user_ids(&engine);
    std::thread::Builder::new()
        .name("detect".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            let mut sys = System::new();
            let mut seen: HashMap<u32, Seen> = HashMap::new();
            std::thread::sleep(Duration::from_secs(2));
            loop {
                // Normally every 6 s; several times a second only while a just-opened window hasn't
                // been identified (at most ~90 s per new window).
                let fast = engine.detect_pass(&mut sys, &mut seen);
                std::thread::sleep(if fast { Duration::from_millis(300) } else { Duration::from_secs(6) });
            }
        })
        .expect("spawn detect thread");
}

#[cfg(test)]
mod tests {
    use super::find_user_id;
    #[test]
    fn parses_log_names_and_user_ids() {
        let line = "Report game_join_loadtime: placeid:13379208636, join_time:1.02, userid:11780444860, ";
        assert_eq!(find_user_id(line), Some(11780444860));
        assert_eq!(find_user_id("no id here"), None);
    }
}
