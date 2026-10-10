use super::winscan;
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

fn alive(pid: u32) -> bool {
    let mut s = System::new();
    s.refresh_processes_specifics(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true, ProcessRefreshKind::new());
    s.process(Pid::from_u32(pid)).is_some()
}

/// Processes started by `pid` (e.g. the RobloxCrashHandler.exe every client launches). Read BEFORE
/// the client exits: once it's gone, Windows no longer links them to it.
pub fn children_of(pid: u32) -> Vec<u32> {
    let mut s = System::new();
    s.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::new());
    s.processes()
        .values()
        .filter(|p| p.parent() == Some(Pid::from_u32(pid)))
        .map(|p| p.pid().as_u32())
        .collect()
}

/// Kill and wait (up to ~3 s) until the process is really gone. Returns whether it is.
fn end_process(pid: u32) -> bool {
    for _ in 0..15 {
        let mut s = System::new();
        s.refresh_processes_specifics(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true, ProcessRefreshKind::new());
        match s.process(Pid::from_u32(pid)) {
            None => return true,
            Some(p) => {
                p.kill();
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    !alive(pid)
}

/// End a client right now (no polite close) together with what it started. For a duplicate window
/// that must be gone before it can get the account's other window kicked.
pub fn end_now(pid: u32) -> bool {
    let kids = children_of(pid);
    let gone = end_process(pid);
    for k in kids {
        end_process(k);
    }
    gone
}

/// Stop a client completely: error dialog(s) WM_CLOSE -> 2 s -> main window WM_CLOSE -> 2 s -> force
/// end, then end whatever it started (crash handler) so nothing is left using memory.
/// Returns false only if Windows refused to end the client. Blocking; call from a std thread or
/// spawn_blocking.
pub fn graceful_close(pid: u32) -> bool {
    let kids = children_of(pid);
    let roots = winscan::error_roots(&winscan::scan(&[pid]), pid);
    if !roots.is_empty() {
        roots.into_iter().for_each(winscan::close);
        std::thread::sleep(Duration::from_secs(2));
    }
    if alive(pid) {
        for h in winscan::top_levels_of(&winscan::scan(&[pid]), pid) {
            winscan::close(h);
        }
        for _ in 0..10 {
            std::thread::sleep(Duration::from_millis(200));
            if !alive(pid) {
                break;
            }
        }
    }
    let gone = !alive(pid) || end_process(pid);
    for k in kids {
        end_process(k);
    }
    gone
}

// ---------------- log tail ----------------
pub struct LogTail {
    pub path: PathBuf,
    pos: u64,
}

pub fn logs_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var("LOCALAPPDATA").ok()?).join("Roblox").join("logs"))
}

impl LogTail {
    /// Follow a specific log from byte `pos` (0 = from the start).
    pub fn at(path: PathBuf, pos: u64) -> LogTail {
        LogTail { path, pos }
    }

    /// The Player log Roblox created for the process that started at `start` (unix s). Every client
    /// writes its own log whose name carries its start time (..._20261009T080341Z_Player_...), so the
    /// closest name within a few seconds is that client's, unambiguous even with many clients.
    pub fn for_process(start: u64, claimed: &HashSet<PathBuf>) -> Option<LogTail> {
        fs::read_dir(logs_dir()?)
            .ok()?
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let ok = name.ends_with(".log") && name.contains("_Player_") && !name.contains("CrashHandler");
                ok.then(|| log_time(&name).map(|t| (t, e.path()))).flatten()
            })
            .filter(|(t, p)| !claimed.contains(p) && *t + 3 >= start && *t <= start + 20)
            .min_by_key(|(t, _)| t.abs_diff(start))
            .map(|(_, path)| LogTail::at(path, 0))
    }

    /// Fallback when no name matches: newest Player log created at/after launch time and not already
    /// claimed by another account.
    pub fn claim(launch_ts: u64, claimed: &HashSet<PathBuf>) -> Option<LogTail> {
        let min = UNIX_EPOCH + Duration::from_secs(launch_ts.saturating_sub(5));
        let mut best: Option<(SystemTime, PathBuf)> = None;
        for e in fs::read_dir(logs_dir()?).ok()?.flatten() {
            let p = e.path();
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if !name.ends_with(".log") || !name.contains("Player") || claimed.contains(&p) {
                continue;
            }
            let Ok(m) = e.metadata() else { continue };
            let Ok(t) = m.created().or_else(|_| m.modified()) else { continue };
            if t >= min && best.as_ref().map_or(true, |(b, _)| t > *b) {
                best = Some((t, p));
            }
        }
        best.map(|(_, path)| LogTail { path, pos: 0 })
    }

    pub fn read_new(&mut self) -> String {
        let Ok(mut f) = fs::File::open(&self.path) else { return String::new() };
        if f.seek(SeekFrom::Start(self.pos)).is_err() {
            return String::new();
        }
        let mut buf = Vec::new();
        if f.read_to_end(&mut buf).is_err() {
            return String::new();
        }
        // Only COMPLETE lines: a line Roblox is still writing stays for next time, so a marker is
        // never missed or misread because it was cut in half between two reads.
        let Some(end) = buf.iter().rposition(|b| *b == b'\n') else { return String::new() };
        buf.truncate(end + 1);
        self.pos += buf.len() as u64;
        String::from_utf8_lossy(&buf).into_owned()
    }
}

/// Expects a line like:  ! Joining game 'GUID' place 123 at 1.2.3.4
/// Only used when same-server rejoin has been verified AND enabled; the result is validated as a GUID.
/// Settings Roblox refused: "[FLog::FlagFetchingStarterModule] Denied local configuration for: X".
pub fn find_denied(t: &str) -> Vec<String> {
    const KEY: &str = "Denied local configuration for: ";
    t.lines().filter_map(|l| l.find(KEY).map(|i| l[i + KEY.len()..].trim().to_string())).filter(|s| !s.is_empty()).collect()
}

/// Universe id from the last "…universeid:383310974…" (written when the client joins a game).
pub fn find_universe(t: &str) -> Option<u64> {
    const KEY: &str = "universeid:";
    let i = t.rfind(KEY)? + KEY.len();
    t[i..].chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().ok().filter(|v| *v > 0)
}

/// Place id from the last "! Joining game '…' place 123 at …" line.
pub fn find_place(t: &str) -> Option<String> {
    let i = t.rfind("! Joining game '")?;
    let rest = &t[i..];
    let j = rest.find("' place ")? + "' place ".len();
    let d: String = rest[j..].chars().take_while(|c| c.is_ascii_digit()).collect();
    (!d.is_empty()).then_some(d)
}

pub fn find_job_id(t: &str) -> Option<String> {
    const KEY: &str = "! Joining game '";
    let i = t.rfind(KEY)? + KEY.len();
    let rest = &t[i..];
    let id = &rest[..rest.find('\'')?];
    (id.len() >= 32 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')).then(|| id.to_string())
}

/// Unix seconds from a log name's "..._20261009T080341Z_..." part.
pub fn log_time(name: &str) -> Option<u64> {
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

/// What a client's log says happened, in order.
#[derive(Debug, PartialEq)]
pub enum LogEvent {
    /// Joined a server (first join, a teleport, or Roblox's own Reconnect button).
    Joined,
    /// Disconnected / kicked / server shut down. Carries Roblox's own reason text.
    Disconnected(String),
    /// Left the game and is back on the Roblox home screen.
    LeftToHome,
}

/// Markers taken from real Roblox Player logs, e.g.
///   [FLog::Network] Client has been disconnected with reason: Lost connection to the game server, please reconnect
///   [FLog::Network] Lost connection with reason : Lost connection to the game server, please reconnect
///   [DFLog::RbxTransportDummyClient] Disconnected from server for reason: Player: 285 (DisconnectClientInitiated)
///   [FLog::SingleSurfaceApp] returnToLuaApp: (stage:UGCGame).
/// Every server-side end of a session (lost connection 277, kicked 267, server shut down, banned,
/// same account joined elsewhere 273, idle kick, ...) goes through the "disconnected with reason"
/// path; only the reason text differs, and it's kept for the Activity log.
const DISCONNECT_KEYS: [&str; 6] = [
    "client has been disconnected with reason",
    "lost connection with reason",
    "sending disconnect with reason",
    "disconnection notification",
    "you have been kicked",
    "kicked from this experience",
];

pub fn scan_events(text: &str) -> Vec<LogEvent> {
    let mut out = vec![];
    for line in text.lines() {
        let l = line.to_ascii_lowercase();
        if l.contains("! joining game '") || l.contains("game_join_loadtime") {
            if out.last() != Some(&LogEvent::Joined) {
                out.push(LogEvent::Joined);
            }
        } else if let Some(k) = DISCONNECT_KEYS.iter().find(|k| l.contains(*k)) {
            out.push(LogEvent::Disconnected(reason_after(line, &l, k)));
        } else if l.contains("disconnected from server for reason") && !l.contains("disconnectclientinitiated") {
            // Client-initiated (285) is the player leaving or the window closing: not an error by
            // itself; leaving to the home screen is reported separately below.
            out.push(LogEvent::Disconnected(reason_after(line, &l, "disconnected from server for reason")));
        } else if l.contains("returntoluaapp: (stage:ugcgame)") {
            out.push(LogEvent::LeftToHome);
        }
    }
    out
}

/// Roblox's reason text after `key` ("...with reason: Lost connection..." -> "Lost connection...").
fn reason_after(line: &str, lower: &str, key: &str) -> String {
    let i = lower.find(key).map(|i| i + key.len()).unwrap_or(0);
    let r: String = line[i..].trim_start_matches([' ', ':', '.']).trim().trim_end_matches(',').chars().take(140).collect();
    if r.is_empty() {
        "disconnected".into()
    } else {
        r
    }
}

#[cfg(test)]
mod tests {
    use super::{log_time, scan_events, LogEvent::*};

    #[test]
    fn reads_a_real_disconnect_reconnect_leave_sequence() {
        // Trimmed from a real Player log.
        let log = concat!(
            "2026-10-08T16:59:22.053Z,2.05,7c44,6 [FLog::Output] ! Joining game 'b77a7f4f-c10f-4b67-b65d-32f3eab3f768' place 13379208636 at 10.32.33.200\n",
            "2026-10-08T16:59:22.053Z,2.05,7c44,6 [FLog::GameJoinLoadTime] Report game_join_loadtime: placeid:13379208636, userid:11780444860,\n",
            "2026-10-08T16:59:49.252Z,29.25,7c44,7 [FLog::Network] Client has been disconnected with reason: Lost connection to the game server, please reconnect\n",
            "2026-10-08T16:59:49.274Z,29.27,7c44,7,Warning [FLog::Network] Lost connection with reason : Lost connection to the game server, please reconnect\n",
            "2026-10-08T16:59:58.723Z,38.72,3bec,6,Info [DFLog::NetworkClient] Client:Disconnect\n",
            "2026-10-08T16:59:59.092Z,39.09,17a4,6 [FLog::Output] ! Joining game 'c51ff09f-f1a4-494c-a337-b19655f6ced1' place 13379208636 at 10.32.33.200\n",
            "2026-10-08T17:00:23.620Z,63.62,7be8,6,Info [DFLog::RbxTransportDummyClient] Disconnected from server for reason: Player: 285 (DisconnectClientInitiated)\n",
            "2026-10-08T17:00:23.627Z,63.62,7be8,6 [FLog::SingleSurfaceApp] returnToLuaApp: (stage:UGCGame).\n",
        );
        let lost = "Lost connection to the game server, please reconnect".to_string();
        assert_eq!(scan_events(log), vec![Joined, Disconnected(lost.clone()), Disconnected(lost), Joined, LeftToHome]);
    }

    #[test]
    fn kicks_and_server_reasons() {
        let ev = scan_events("x [FLog::Network] Client has been disconnected with reason: You were kicked from this experience: AFK\n");
        assert_eq!(ev, vec![Disconnected("You were kicked from this experience: AFK".into())]);
        let ev = scan_events("x Disconnected from server for reason: Server: 288 (ServerShutdown)\n");
        assert_eq!(ev, vec![Disconnected("Server: 288 (ServerShutdown)".into())]);
        assert!(scan_events("x [DFLog::NetworkClient] Client:Disconnect\n").is_empty());
    }

    #[test]
    fn log_names() {
        assert_eq!(log_time("0.742.0.7421053_20261009T080341Z_Player_02B48_last.log"), Some(1791533021));
    }
}

/// Dev check, not run by default: `cargo test -p roblox_account_manager replay_local_logs -- --ignored --nocapture`
/// replays every Roblox Player log on this PC through scan_events and prints what it would act on.
#[cfg(test)]
#[test]
#[ignore]
fn replay_local_logs() {
    let Some(dir) = logs_dir() else { return };
    for e in fs::read_dir(dir).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.contains("_Player_") || name.contains("CrashHandler") {
            continue;
        }
        let text = String::from_utf8_lossy(&fs::read(e.path()).unwrap_or_default()).into_owned();
        let ev = scan_events(&text);
        // Same rule as the watcher: a disconnect/leave only counts if no join follows it.
        let last_problem = ev.iter().rposition(|x| !matches!(x, LogEvent::Joined));
        let rejoined_after = last_problem.map_or(false, |i| ev[i + 1..].iter().any(|x| matches!(x, LogEvent::Joined)));
        let verdict = match last_problem.map(|i| &ev[i]) {
            None => "no problem".to_string(),
            Some(_) if rejoined_after => "problem, then rejoined (no action)".to_string(),
            Some(p) => format!("WOULD RECOVER: {p:?}"),
        };
        println!("{name}: {} events -> {verdict}", ev.len());
    }
}

/// Stand-in for a Roblox client + its crash handler: cmd.exe running a long ping (a child process).
/// `cargo test -p roblox_account_manager stop_ends_process_and_children -- --ignored`
#[cfg(test)]
#[test]
#[ignore]
fn stop_ends_process_and_children() {
    let parent = std::process::Command::new("cmd").args(["/c", "ping -n 60 127.0.0.1 >nul"]).spawn().unwrap();
    let pid = parent.id();
    std::thread::sleep(Duration::from_millis(800));
    let kids = children_of(pid);
    assert!(!kids.is_empty(), "the stand-in should have a child process");
    let t = std::time::Instant::now();
    assert!(graceful_close(pid), "parent should be ended");
    assert!(!alive(pid), "parent still running");
    for k in &kids {
        assert!(!alive(*k), "child {k} still running");
    }
    println!("ended parent {pid} and children {kids:?} in {:.1} s", t.elapsed().as_secs_f32());
}
