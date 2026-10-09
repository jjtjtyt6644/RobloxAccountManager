//! Read-only memory figures for this process (Settings → Diagnostics, and the 5-minute sample in Activity).
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::GetCurrentProcess;

#[derive(Clone, Copy, Default)]
pub struct Mem {
    /// Resident set (what Task Manager's "Memory" column approximates).
    pub working_set: u64,
    pub peak_working_set: u64,
    /// Committed private bytes.
    pub private: u64,
}

pub fn current() -> Option<Mem> {
    unsafe {
        let cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let mut c = PROCESS_MEMORY_COUNTERS { cb, ..Default::default() };
        GetProcessMemoryInfo(GetCurrentProcess(), &mut c, cb).ok()?;
        Some(Mem {
            working_set: c.WorkingSetSize as u64,
            peak_working_set: c.PeakWorkingSetSize as u64,
            private: c.PagefileUsage as u64,
        })
    }
}

pub fn mb(b: u64) -> String {
    format!("{:.1} MB", b as f64 / (1024.0 * 1024.0))
}
