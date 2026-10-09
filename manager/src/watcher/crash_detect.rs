use super::winscan;
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, ProcessesToUpdate, System};

fn alive(pid: u32) -> bool {
    let mut s = System::new();
    s.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
    s.process(Pid::from_u32(pid)).is_some()
}

/// error dialog(s) WM_CLOSE -> 2s -> main window WM_CLOSE -> 2s -> force kill.
/// Blocking; always call from a std thread or spawn_blocking.
pub fn graceful_close(pid: u32) {
    let roots = winscan::error_roots(&winscan::scan(&[pid]), pid);
    if !roots.is_empty() {
        roots.into_iter().for_each(winscan::close);
        std::thread::sleep(Duration::from_secs(2));
    }
    if !alive(pid) {
        return;
    }
    for h in winscan::top_levels_of(&winscan::scan(&[pid]), pid) {
        winscan::close(h);
    }
    std::thread::sleep(Duration::from_secs(2));
    let mut s = System::new();
    s.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
    if let Some(p) = s.process(Pid::from_u32(pid)) {
        p.kill();
    }
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
    /// Newest Player log created at/after launch time and not already claimed by another account.
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
        self.pos += buf.len() as u64;
        String::from_utf8_lossy(&buf).into_owned()
    }
}

/// Expects a line like:  ! Joining game 'GUID' place 123 at 1.2.3.4
/// Only used when same-server rejoin has been verified AND enabled; the result is validated as a GUID.
pub fn find_job_id(t: &str) -> Option<String> {
    const KEY: &str = "! Joining game '";
    let i = t.rfind(KEY)? + KEY.len();
    let rest = &t[i..];
    let id = &rest[..rest.find('\'')?];
    (id.len() >= 32 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')).then(|| id.to_string())
}

pub fn has_disconnect(t: &str) -> bool {
    let l = t.to_lowercase();
    ["sending disconnect with reason", "disconnection notification", "connection lost"].iter().any(|k| l.contains(k))
}
