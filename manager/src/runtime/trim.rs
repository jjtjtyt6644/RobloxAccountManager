//! Background saver for *Roblox clients*: memory (Settings → Memory saver) and CPU (Settings → CPU
//! saver). The manager's own background behaviour lives in runtime::selfmode.
//!
//! Every client is either the one in the foreground (the window you're playing in) or background.
//! The foreground client is always left alone: limits are lifted, memory priority, CPU priority,
//! Efficiency mode and core limits are restored the moment it becomes foreground (checked every
//! second, so switching windows never leaves you playing a throttled client for long).
//!
//! Memory, for background clients:
//!   Balanced  every 15 s  memory priority low; trimmed (EmptyWorkingSet) only if above the target.
//!   Strong    every 5 s   same, checked more often so they stay near the target.
//!   Max       every 3 s   same, plus a HARD working-set ceiling at the target. Windows then keeps the
//!                         client at or under the target at all times; the cost is page faults (CPU,
//!                         and disk if the pages were written out), so the client may run slowly.
//!   Trimmed pages move to the standby list or the page file: RAM really is free for other programs,
//!   but a client that needs those pages again faults them back in.
//!
//! CPU, for background clients:
//!   Efficiency  Windows Efficiency mode (EcoQoS — lower clocks / efficiency cores, and the client's
//!               1 ms timer requests are ignored) and below-normal priority. This is what Task
//!               Manager's "Efficiency mode" does.
//!   Strong      Efficiency + idle priority + at most 2 CPU cores (the highest-numbered ones, which
//!               on hybrid Intel CPUs are the efficiency cores).
//!
//! The loop only runs while clients are live and a saver is on; otherwise it sleeps.
use crate::launcher::Engine;
use crate::runtime::priority;
use crate::storage::db::{CpuMode, TrimMode};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Memory::{
    SetProcessWorkingSetSizeEx, QUOTA_LIMITS_HARDWS_MAX_DISABLE, QUOTA_LIMITS_HARDWS_MAX_ENABLE,
    QUOTA_LIMITS_HARDWS_MIN_DISABLE,
};
use windows::Win32::System::ProcessStatus::{EmptyWorkingSet, GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetProcessAffinityMask, OpenProcess, ProcessMemoryPriority, ProcessPowerThrottling,
    SetProcessAffinityMask, SetProcessInformation, MEMORY_PRIORITY, MEMORY_PRIORITY_INFORMATION, MEMORY_PRIORITY_LOW,
    MEMORY_PRIORITY_NORMAL, PROCESS_INFORMATION_CLASS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
    PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, PROCESS_SET_QUOTA,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

const MB: u64 = 1024 * 1024;
/// Windows' own default working-set limits (soft). Restoring these removes our hard ceiling without
/// emptying the working set the way (-1, -1) would.
const DEFAULT_MIN_WS: usize = 200 * 4096;
const DEFAULT_MAX_WS: usize = 345 * 4096;
const STRONG_CORES: u32 = 2;

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

pub struct Proc(HANDLE);

impl Proc {
    pub fn open(pid: u32) -> Option<Proc> {
        unsafe {
            OpenProcess(PROCESS_SET_QUOTA | PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
                .ok()
                .map(Proc)
        }
    }

    /// This process (pseudo-handle; closing it is a no-op).
    pub fn current() -> Proc {
        Proc(unsafe { GetCurrentProcess() })
    }

    pub fn working_set(&self) -> Option<u64> {
        let cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let mut c = PROCESS_MEMORY_COUNTERS { cb, ..Default::default() };
        unsafe { GetProcessMemoryInfo(self.0, &mut c, cb).ok()? };
        Some(c.WorkingSetSize as u64)
    }

    pub fn trim(&self) -> bool {
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

    fn set_info<T>(&self, class: PROCESS_INFORMATION_CLASS, v: &T) {
        unsafe {
            let _ = SetProcessInformation(self.0, class, v as *const T as *const _, std::mem::size_of::<T>() as u32);
        }
    }

    fn set_memory_priority(&self, p: MEMORY_PRIORITY) {
        self.set_info(ProcessMemoryPriority, &MEMORY_PRIORITY_INFORMATION { MemoryPriority: p });
    }

    /// Windows Efficiency mode (EcoQoS) on/off. Also stops the process's high-resolution timer
    /// requests from keeping the whole CPU awake.
    pub fn set_efficiency(&self, on: bool) {
        let bits = PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION;
        self.set_info(
            ProcessPowerThrottling,
            &PROCESS_POWER_THROTTLING_STATE {
                Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                ControlMask: bits,
                StateMask: if on { bits } else { 0 },
            },
        );
    }

    fn set_affinity(&self, mask: usize) {
        if mask != 0 {
            unsafe {
                let _ = SetProcessAffinityMask(self.0, mask);
            }
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

/// (all cores the system lets us use, the top `n` of them).
fn core_masks(n: u32) -> (usize, usize) {
    let (mut mine, mut system) = (0usize, 0usize);
    unsafe {
        let _ = GetProcessAffinityMask(GetCurrentProcess(), &mut mine, &mut system);
    }
    let mut limited = 0usize;
    let mut left = n;
    for bit in (0..usize::BITS).rev() {
        if left == 0 {
            break;
        }
        if system & (1usize << bit) != 0 {
            limited |= 1usize << bit;
            left -= 1;
        }
    }
    (system, limited)
}

/// What we've currently applied to one client, so we only touch it when something changes and can
/// undo exactly what we did.
#[derive(Default, Clone, Copy, PartialEq)]
struct Applied {
    background: bool,
    cpu: Option<CpuMode>,
    low_mem_prio: bool,
    ceiling: bool,
}

pub fn spawn(engine: Engine) {
    std::thread::Builder::new()
        .name("client-saver".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            let (all_cores, few_cores) = core_masks(STRONG_CORES);
            let mut applied: HashMap<u32, Applied> = HashMap::new();
            let mut tick: u64 = 0;
            loop {
                let (mem_mode, cpu_mode, target, all_windows) = {
                    let s = engine.settings.read().unwrap();
                    (s.trim_mode, s.cpu_saver, s.trim_target_mb as u64 * MB, s.saver_all_windows)
                };
                // Tracked clients, plus Roblox windows the manager didn't start (id -1: no account,
                // normal priority to restore).
                let mut clients: Vec<(i64, u32, i32)> =
                    engine.tr().iter().filter_map(|(id, t)| t.pid.map(|p| (*id, p, t.base_priority))).collect();
                clients.extend(engine.others.lock().unwrap().iter().map(|o| (-1, o.pid, 0)));
                let live: HashSet<u32> = clients.iter().map(|c| c.1).collect();
                applied.retain(|p, _| live.contains(p));

                // Nothing to do: sleep longer, but first undo anything still applied (saver just turned off).
                if mem_mode == TrimMode::Off && cpu_mode == CpuMode::Off {
                    for (&pid, a) in applied.iter() {
                        if let Some(p) = Proc::open(pid) {
                            if a.ceiling {
                                p.set_ceiling(None);
                            }
                            if a.low_mem_prio {
                                p.set_memory_priority(MEMORY_PRIORITY_NORMAL);
                            }
                            if a.cpu.is_some() {
                                p.set_efficiency(false);
                                p.set_affinity(all_cores);
                            }
                        }
                    }
                    if !applied.is_empty() {
                        for (_, pid, base) in &clients {
                            priority::apply(*pid, *base);
                        }
                        engine.tr().values_mut().for_each(|e| e.mem_note = MemNote::None);
                        applied.clear();
                    }
                    std::thread::sleep(Duration::from_secs(3));
                    continue;
                }
                std::thread::sleep(Duration::from_secs(1));
                if clients.is_empty() {
                    continue;
                }
                tick += 1;
                let trim_now = match mem_mode {
                    TrimMode::Off => false,
                    TrimMode::Balanced => tick % 15 == 0,
                    TrimMode::Strong => tick % 5 == 0,
                    TrimMode::Max => tick % 3 == 0,
                };

                let fg = foreground_pid();
                let mut notes: Vec<(i64, Option<u64>, MemNote)> = Vec::new();
                for (id, pid, base) in clients {
                    // "background" = gets the savers. With "every window" on, so does the one you're playing.
                    let background = all_windows || Some(pid) != fg;
                    let was = applied.get(&pid).copied().unwrap_or_default();
                    let mut now = was;
                    now.background = background;
                    let first = !applied.contains_key(&pid);
                    let mem_on = mem_mode != TrimMode::Off;
                    let want_cpu = if background && cpu_mode != CpuMode::Off { Some(cpu_mode) } else { None };
                    let want_low = background && mem_on;
                    let want_ceiling = background && mem_mode == TrimMode::Max;
                    // Nothing changed and nothing periodic due: don't even open the process.
                    let same = was.background == background
                        && was.cpu == want_cpu
                        && was.low_mem_prio == want_low
                        && was.ceiling == want_ceiling;
                    if !first && same && !trim_now {
                        continue;
                    }
                    let Some(p) = Proc::open(pid) else { continue };

                    // ---- CPU ----
                    if want_cpu != was.cpu || first {
                        match want_cpu {
                            Some(m) => {
                                p.set_efficiency(true);
                                let tier = if m == CpuMode::Strong { 2 } else { base.max(1) };
                                priority::apply(pid, tier);
                                p.set_affinity(if m == CpuMode::Strong { few_cores } else { all_cores });
                            }
                            None => {
                                if was.cpu.is_some() {
                                    p.set_efficiency(false);
                                    p.set_affinity(all_cores);
                                    priority::apply(pid, base);
                                }
                            }
                        }
                        now.cpu = want_cpu;
                    }

                    // ---- memory ----
                    if want_low != was.low_mem_prio {
                        p.set_memory_priority(if want_low { MEMORY_PRIORITY_LOW } else { MEMORY_PRIORITY_NORMAL });
                        now.low_mem_prio = want_low;
                    }
                    if want_ceiling && (trim_now || !was.ceiling) {
                        // Re-applied each pass so a changed target takes effect.
                        now.ceiling = p.set_ceiling(Some(target));
                    } else if !want_ceiling && was.ceiling {
                        p.set_ceiling(None);
                        now.ceiling = false;
                    }
                    if mem_on {
                        let note = if !background {
                            MemNote::Focused
                        } else if trim_now {
                            let ws = p.working_set().unwrap_or(u64::MAX);
                            if ws <= target {
                                if now.ceiling { MemNote::Limited } else { MemNote::UnderTarget }
                            } else if p.trim() {
                                if now.ceiling { MemNote::Limited } else { MemNote::Trimmed }
                            } else {
                                MemNote::Refused
                            }
                        } else {
                            engine.tr().get(&id).map(|t| t.mem_note).unwrap_or_default()
                        };
                        notes.push((id, p.working_set(), note));
                    }
                    applied.insert(pid, now);
                }
                if !notes.is_empty() {
                    let mut t = engine.tr();
                    for (id, ws, note) in notes {
                        if let Some(e) = t.get_mut(&id) {
                            if let Some(ws) = ws {
                                e.mem_ws = ws;
                            }
                            e.mem_note = note;
                        }
                    }
                    drop(t);
                    engine.ui.repaint();
                }
            }
        })
        .expect("spawn client-saver thread");
}
