use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
    OpenProcess, SetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
    PROCESS_SET_INFORMATION,
};

/// tier: 0 Normal (main), 1 BelowNormal (alt), 2 Idle
pub fn apply(pid: u32, tier: i32) {
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SET_INFORMATION, false, pid) {
            let class = match tier {
                0 => NORMAL_PRIORITY_CLASS,
                1 => BELOW_NORMAL_PRIORITY_CLASS,
                _ => IDLE_PRIORITY_CLASS,
            };
            let _ = SetPriorityClass(h, class);
            let _ = CloseHandle(h);
        }
    }
}
