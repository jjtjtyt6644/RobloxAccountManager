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
use super::crash_detect::{logs_dir, LogTail};
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

/// Unix seconds from a log name's "20261009T080341Z" part.
fn log_time(name: &str) -> Option<u64> {
    let i = name.find('_')? + 1;
    let t = name.get(i..i + 16)?;
    if t.as_bytes()[8] != b'T' || !t.ends_with('Z') {
        return None;
    }
    let n = |a: usize, b: usize| t.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, s) = (n(0, 4)?, n(4, 6)?, n(6, 8)?, n(9, 11)?, n(11, 13)?, n(13, 15)?);
    // Days from civil date (Howard Hinnant's algorithm).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(days * 86400 + h * 3600 + mi * 60 + s).ok()
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
}

impl Engine {
    /// One detection pass. `seen` persists between passes.
    fn detect_pass(&self, sys: &mut System, seen: &mut HashMap<u32, Seen>) {
        // A launch in progress may be about to bind a brand-new client; let it, then look again.
        let busy = self.tr().values().any(|t| matches!(t.state, State::Starting | State::Reconnecting));
        if busy {
            return;
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
                let s = seen.entry(pid).or_insert(Seen { start: st, log: None, user_id: None });
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
                        s.user_id = find_user_id(&l.read_new());
                    }
                }
            }
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
    }
}

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
                engine.detect_pass(&mut sys, &mut seen);
                std::thread::sleep(Duration::from_secs(6));
            }
        })
        .expect("spawn detect thread");
}

#[cfg(test)]
mod tests {
    use super::{find_user_id, log_time};
    #[test]
    fn parses_log_names_and_user_ids() {
        // 2026-10-09T08:03:41Z
        assert_eq!(log_time("0.742.0.7421053_20261009T080341Z_Player_02B48_last.log"), Some(1791533021));
        assert_eq!(log_time("garbage.log"), None);
        let line = "Report game_join_loadtime: placeid:13379208636, join_time:1.02, userid:11780444860, ";
        assert_eq!(find_user_id(line), Some(11780444860));
        assert_eq!(find_user_id("no id here"), None);
    }
}
