//! Memory saver for *Roblox clients* only (the manager never trims itself — see CHANGES.md §5).
//!
//! Each pass looks at every live client and splits them into the one in the foreground (the window
//! you're playing in) and the rest. The foreground client is always left alone: any limit we set is
//! lifted and its memory priority is restored, so the game you're playing never stutters because of
//! this. Background clients, depending on the level:
//!
//!   Balanced  every 15 s  memory priority low; trimmed (EmptyWorkingSet) only if above the target.
//!   Strong    every 5 s   same, checked more often so they stay near the target.
//!   Max       every 3 s   same, plus a HARD working-set ceiling at the target. Windows then keeps the
//!                         client at or under the target at all times; the cost is page faults (CPU,
//!                         and disk if the pages were written out), so the client may run slowly while
//!                         in the background. The ceiling is removed the moment it becomes foreground.
//!
//! Low memory priority means that when the PC does run short, Windows takes pages from these clients
//! first and drops them from the standby list first — it costs nothing until memory is actually tight.
//!
//! Honest caveat (shown in Settings): trimmed pages move to the standby list or the page file. Task
//! Manager's number drops and RAM is free for other programs, but a client that needs those pages
//! again faults them back in. ~100 MB is realistic for an alt on a light preset; a heavy game on a
//! full preset rebounds between passes unless Max is used.
use crate::launcher::Engine;
use crate::storage::db::TrimMode;
use std::collections::HashSet;
use std::time::Duration;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Memory::{
    SetProcessWorkingSetSizeEx, QUOTA_LIMITS_HARDWS_MAX_DISABLE, QUOTA_LIMITS_HARDWS_MAX_ENABLE,
    QUOTA_LIMITS_HARDWS_MIN_DISABLE,
};
use windows::Win32::System::ProcessStatus::{EmptyWorkingSet, GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::{
    OpenProcess, ProcessMemoryPriority, SetProcessInformation, MEMORY_PRIORITY, MEMORY_PRIORITY_INFORMATION,
    MEMORY_PRIORITY_LOW, MEMORY_PRIORITY_NORMAL, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
    PROCESS_SET_QUOTA,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

const MB: u64 = 1024 * 1024;
/// Windows' own default working-set limits (soft). Restoring these removes our hard ceiling without
/// emptying the working set the way (-1, -1) would.
const DEFAULT_MIN_WS: usize = 200 * 4096;
const DEFAULT_MAX_WS: usize = 345 * 4096;

/// What the memory saver last did to a client (shown on its row / Session card).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MemNote {
    #[default]
    None,
    /// Foreground window: deliberately left alone.
    Focused,
    /// Background and already at or under the target.
    UnderTarget,
    Trimmed,
    /// Background with a hard ceiling (Max).
    Limited,
    /// Windows refused (access denied).
    Refused,
}

impl MemNote {
    pub fn text(self) -> &'static str {
        match self {
            MemNote::None => "",
            MemNote::Focused => "In focus — left alone",
            MemNote::UnderTarget => "Under target",
            MemNote::Trimmed => "Trimmed",
            MemNote::Limited => "Limited to target",
            MemNote::Refused => "Windows refused to trim it",
        }
    }
}

struct Proc(HANDLE);

impl Proc {
    fn open(pid: u32) -> Option<Proc> {
        unsafe {
            OpenProcess(PROCESS_SET_QUOTA | PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
                .ok()
                .map(Proc)
        }
    }

    fn working_set(&self) -> Option<u64> {
        let cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let mut c = PROCESS_MEMORY_COUNTERS { cb, ..Default::default() };
        unsafe { GetProcessMemoryInfo(self.0, &mut c, cb).ok()? };
        Some(c.WorkingSetSize as u64)
    }

    fn trim(&self) -> bool {
        unsafe { EmptyWorkingSet(self.0).is_ok() }
    }

    /// Some(bytes) = hard ceiling; None = back to Windows' soft defaults.
    fn set_ceiling(&self, max: Option<u64>) -> bool {
        unsafe {
            match max {
                Some(max) => {
                    let max = max.max(32 * MB) as usize;
                    SetProcessWorkingSetSizeEx(
                        self.0,
                        (max / 4).max(DEFAULT_MIN_WS),
                        max,
                        QUOTA_LIMITS_HARDWS_MAX_ENABLE | QUOTA_LIMITS_HARDWS_MIN_DISABLE,
                    )
                    .is_ok()
                }
                None => SetProcessWorkingSetSizeEx(
                    self.0,
                    DEFAULT_MIN_WS,
                    DEFAULT_MAX_WS,
                    QUOTA_LIMITS_HARDWS_MAX_DISABLE | QUOTA_LIMITS_HARDWS_MIN_DISABLE,
                )
                .is_ok(),
            }
        }
    }

    fn set_memory_priority(&self, p: MEMORY_PRIORITY) {
        let info = MEMORY_PRIORITY_INFORMATION { MemoryPriority: p };
        unsafe {
            let _ = SetProcessInformation(
                self.0,
                ProcessMemoryPriority,
                &info as *const _ as *const _,
                std::mem::size_of::<MEMORY_PRIORITY_INFORMATION>() as u32,
            );
        }
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn foreground_pid() -> Option<u32> {
    unsafe {
        let h = GetForegroundWindow();
        if h.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        (pid != 0).then_some(pid)
    }
}

pub fn spawn(engine: Engine) {
    std::thread::Builder::new()
        .name("client-trim".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            // Clients we've changed, so we can undo exactly that (and nothing else).
            let mut ceiling: HashSet<u32> = HashSet::new();
            let mut low_prio: HashSet<u32> = HashSet::new();
            loop {
                let mode = engine.settings.read().unwrap().trim_mode;
                std::thread::sleep(Duration::from_secs(match mode {
                    TrimMode::Off => 5,
                    TrimMode::Balanced => 15,
                    TrimMode::Strong => 5,
                    TrimMode::Max => 3,
                }));
                // Re-read: the level may have changed while we slept.
                let (mode, target) = {
                    let s = engine.settings.read().unwrap();
                    (s.trim_mode, s.trim_target_mb as u64 * MB)
                };
                let clients: Vec<(i64, u32)> =
                    engine.tr().iter().filter_map(|(id, t)| t.pid.map(|p| (*id, p))).collect();
                let live: HashSet<u32> = clients.iter().map(|c| c.1).collect();
                ceiling.retain(|p| live.contains(p));
                low_prio.retain(|p| live.contains(p));

                if mode == TrimMode::Off {
                    for pid in ceiling.drain().chain(low_prio.drain()).collect::<HashSet<_>>() {
                        if let Some(p) = Proc::open(pid) {
                            p.set_ceiling(None);
                            p.set_memory_priority(MEMORY_PRIORITY_NORMAL);
                        }
                    }
                    let mut t = engine.tr();
                    t.values_mut().for_each(|e| e.mem_note = MemNote::None);
                    continue;
                }
                if clients.is_empty() {
                    continue;
                }

                let fg = foreground_pid();
                let mut results: Vec<(i64, Option<u64>, MemNote)> = Vec::with_capacity(clients.len());
                for (id, pid) in clients {
                    let Some(p) = Proc::open(pid) else { continue };
                    let note = if Some(pid) == fg {
                        if ceiling.remove(&pid) {
                            p.set_ceiling(None);
                        }
                        if low_prio.remove(&pid) {
                            p.set_memory_priority(MEMORY_PRIORITY_NORMAL);
                        }
                        MemNote::Focused
                    } else {
                        if low_prio.insert(pid) {
                            p.set_memory_priority(MEMORY_PRIORITY_LOW);
                        }
                        if mode == TrimMode::Max {
                            // Re-applied every pass so a changed target takes effect.
                            if p.set_ceiling(Some(target)) {
                                ceiling.insert(pid);
                            }
                        } else if ceiling.remove(&pid) {
                            p.set_ceiling(None);
                        }
                        let ws = p.working_set().unwrap_or(u64::MAX);
                        if ws <= target {
                            if ceiling.contains(&pid) { MemNote::Limited } else { MemNote::UnderTarget }
                        } else if p.trim() {
                            if ceiling.contains(&pid) { MemNote::Limited } else { MemNote::Trimmed }
                        } else {
                            MemNote::Refused
                        }
                    };
                    results.push((id, p.working_set(), note));
                }
                {
                    let mut t = engine.tr();
                    for (id, ws, note) in results {
                        if let Some(e) = t.get_mut(&id) {
                            if let Some(ws) = ws {
                                e.mem_ws = ws;
                            }
                            e.mem_note = note;
                        }
                    }
                }
                engine.ui.ctx.request_repaint();
            }
        })
        .expect("spawn trim thread");
}
