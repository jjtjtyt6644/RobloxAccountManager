//! Window discovery for tracked Roblox clients.
//!
//! For every tracked client we know its "family" (the client PID plus its direct child processes,
//! e.g. a crash handler; cached for 10 s), then collect windows two ways:
//!   1. EnumWindows filtered to family PIDs — every top-level window, owned dialogs included.
//!   2. EnumChildWindows under each of those — finds embedded panels and the static-text controls
//!      inside dialogs, which is where "Disconnected"/"error" text usually lives.
//!
//! Windows from processes outside a client's family are never attributed to it, so one crash dialog
//! can no longer mark every running client as failed.
//!
//! Note: Roblox's in-game disconnect screen (e.g. "Error Code: 277") is drawn by the engine inside the
//! game surface and is NOT a Win32 window. No amount of window walking sees it; the client's log is
//! what catches those (crash_detect::scan_events).
use std::collections::{HashMap, HashSet};
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, WPARAM};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetWindowThreadProcessId,
    InternalGetWindowText, IsHungAppWindow, IsIconic, IsWindowVisible, PostMessageW, SendMessageTimeoutW,
    SetForegroundWindow, ShowWindow, SMTO_ABORTIFHUNG, SW_RESTORE, WM_CLOSE, WM_SETTEXT,
};

pub struct Win {
    pub hwnd: HWND,
    /// The top-level window this one lives under (itself for top-level windows). This is what we close.
    pub root: HWND,
    pub pid: u32,
    /// Tracked client PID this window is attributed to.
    pub owner: u32,
    pub class: String,
    pub text: String,
    pub top_level: bool,
    pub visible: bool,
}

/// Lower-cased substrings that mark an error/disconnect dialog.
const KEYWORDS: [&str; 9] = [
    "disconnected",
    "lost connection",
    "connection error",
    "crash",
    "error",
    "kicked",
    "unexpected",
    "failed to",
    "another instance",
];

unsafe extern "system" fn push_hwnd(h: HWND, l: LPARAM) -> BOOL {
    let v = &mut *(l.0 as *mut Vec<HWND>);
    v.push(h);
    BOOL(1)
}

fn pid_of(h: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(h, Some(&mut pid));
    }
    pid
}

/// Never sends a message, so a hung client can't stall the watcher (unlike GetWindowText/WM_GETTEXT).
fn text_of(h: HWND) -> String {
    let mut b = [0u16; 512];
    let n = unsafe { InternalGetWindowText(h, &mut b) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&b[..n as usize])
}

fn class_of(h: HWND) -> String {
    let mut b = [0u16; 128];
    let n = unsafe { GetClassNameW(h, &mut b) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&b[..n as usize])
}

thread_local! {
    /// Per calling thread: (when, which clients, family map). A client's child processes are started
    /// with it and rarely change, so the process snapshot is only retaken every 10 s or when the set
    /// of clients changes.
    static FAMILY_CACHE: std::cell::RefCell<Option<(std::time::Instant, Vec<u32>, HashMap<u32, u32>)>> =
        const { std::cell::RefCell::new(None) };
}

fn families(tracked: &[u32]) -> HashMap<u32, u32> {
    let mut key = tracked.to_vec();
    key.sort_unstable();
    FAMILY_CACHE.with(|c| {
        if let Some((at, k, m)) = c.borrow().as_ref() {
            if *k == key && at.elapsed() < std::time::Duration::from_secs(10) {
                return m.clone();
            }
        }
        let m = families_now(tracked);
        *c.borrow_mut() = Some((std::time::Instant::now(), key, m.clone()));
        m
    })
}

/// family pid -> tracked client pid
fn families_now(tracked: &[u32]) -> HashMap<u32, u32> {
    let mut m: HashMap<u32, u32> = tracked.iter().map(|p| (*p, *p)).collect();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return m };
        let mut pe = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &mut pe).is_ok() {
            loop {
                if pe.th32ProcessID != 0 && tracked.contains(&pe.th32ParentProcessID) {
                    m.entry(pe.th32ProcessID).or_insert(pe.th32ParentProcessID);
                }
                if Process32NextW(snap, &mut pe).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    m
}

/// All windows (top-level + descendants) belonging to the given tracked clients.
pub fn scan(tracked: &[u32]) -> Vec<Win> {
    if tracked.is_empty() {
        return Vec::new();
    }
    let fam = families(tracked);
    // EnumWindows lists every top-level window, owned dialogs included, so the old per-thread walk
    // (a snapshot of every thread on the PC, every second) found nothing extra; it's gone.
    let mut tops: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(push_hwnd), LPARAM(&mut tops as *mut _ as isize));
    }
    // Only windows of tracked families matter; skip the rest before reading any text.
    tops.retain(|h| fam.contains_key(&pid_of(*h)));

    let mut seen: HashSet<isize> = HashSet::new();
    let mut out = Vec::new();
    for top in tops {
        if !seen.insert(top.0 as isize) {
            continue;
        }
        let pid = pid_of(top);
        let Some(&owner) = fam.get(&pid) else { continue };
        let top_vis = unsafe { IsWindowVisible(top).as_bool() };
        out.push(Win {
            hwnd: top,
            root: top,
            pid,
            owner,
            class: class_of(top),
            text: text_of(top),
            top_level: true,
            visible: top_vis,
        });
        let mut kids: Vec<HWND> = Vec::new();
        unsafe {
            let _ = EnumChildWindows(top, Some(push_hwnd), LPARAM(&mut kids as *mut _ as isize));
        }
        for k in kids {
            if !seen.insert(k.0 as isize) {
                continue;
            }
            let kpid = pid_of(k);
            out.push(Win {
                hwnd: k,
                root: top,
                pid: kpid,
                owner: fam.get(&kpid).copied().unwrap_or(owner),
                class: class_of(k),
                text: text_of(k),
                top_level: false,
                visible: top_vis && unsafe { IsWindowVisible(k).as_bool() },
            });
        }
    }
    out
}

fn is_error(w: &Win) -> bool {
    if !w.visible || w.text.is_empty() {
        return false;
    }
    if is_main(w) {
        return false; // the normal game window (possibly renamed by us)
    }
    let t = w.text.to_lowercase();
    KEYWORDS.iter().any(|k| t.contains(k))
}

/// Top-level windows (deduplicated) that show error text for this tracked client — either in their
/// own caption or in any descendant control.
pub fn error_roots(ws: &[Win], tracked_pid: u32) -> Vec<HWND> {
    let mut seen = HashSet::new();
    ws.iter()
        .filter(|w| w.owner == tracked_pid && is_error(w))
        .map(|w| w.root)
        .filter(|r| seen.insert(r.0 as isize))
        .collect()
}

/// Short description of the first error found, for the activity log.
pub fn error_summary(ws: &[Win], tracked_pid: u32) -> Option<String> {
    ws.iter().find(|w| w.owner == tracked_pid && is_error(w)).map(|w| {
        let mut t: String = w.text.chars().take(80).collect();
        if w.text.chars().count() > 80 {
            t.push('…');
        }
        let kind = if w.top_level { "dialog" } else { "dialog control" };
        format!("{kind} [{}]: \"{t}\"", w.class)
    })
}

/// The client's own visible main window ("Roblox") exists — i.e. it has finished starting up.
/// Prefix of the titles we give game windows ("Roblox — Main"), so they're told apart on the taskbar.
pub const TITLE_PREFIX: &str = "Roblox — ";

/// A client's game window: titled "Roblox", or renamed by us.
fn is_main(w: &Win) -> bool {
    w.top_level && w.visible && (w.text == "Roblox" || w.text.starts_with(TITLE_PREFIX))
}

pub fn has_main_window(ws: &[Win], pid: u32) -> bool {
    ws.iter().any(|w| w.pid == pid && is_main(w))
}

pub fn main_window(ws: &[Win], pid: u32) -> Option<(HWND, String)> {
    ws.iter().find(|w| w.pid == pid && is_main(w)).map(|w| (w.hwnd, w.text.clone()))
}

/// Retitle a game window. WM_SETTEXT with a timeout, so a frozen client can't stall the caller.
pub fn set_title(h: HWND, title: &str) {
    let wide: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let _ = SendMessageTimeoutW(h, WM_SETTEXT, WPARAM(0), LPARAM(wide.as_ptr() as isize), SMTO_ABORTIFHUNG, 300, None);
    }
}

/// Bring a client's game window to the front (restoring it if minimized).
pub fn focus(pid: u32) -> bool {
    let Some((h, _)) = main_window(&scan(&[pid]), pid) else { return false };
    unsafe {
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        SetForegroundWindow(h).as_bool()
    }
}

/// The client's main window is "not responding" (hasn't handled messages for ~5 s — what Windows
/// shows as "(Not Responding)").
pub fn main_window_hung(ws: &[Win], pid: u32) -> bool {
    ws.iter().filter(|w| w.pid == pid && is_main(w)).any(|w| unsafe { IsHungAppWindow(w.hwnd).as_bool() })
}

pub fn top_levels_of(ws: &[Win], pid: u32) -> Vec<HWND> {
    ws.iter().filter(|w| w.top_level && w.pid == pid).map(|w| w.hwnd).collect()
}

pub fn close(h: HWND) {
    unsafe {
        let _ = PostMessageW(h, WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}
