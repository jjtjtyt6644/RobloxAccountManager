//! Window discovery for tracked Roblox clients.
//!
//! Per tick we build, for every tracked client, its "family" (the client PID plus its direct child
//! processes, e.g. a crash handler), then collect windows three ways:
//!   1. EnumThreadWindows for every thread of every family process — finds each thread's
//!      top-level/owned windows (dialogs) regardless of z-order or whether they have a taskbar entry.
//!   2. EnumWindows filtered to family PIDs — backstop for anything (1) missed.
//!   3. EnumChildWindows under every window from (1)+(2) — finds embedded panels and the static-text
//!      controls inside dialogs, which is where "Disconnected"/"error" text usually lives.
//!
//! Windows from processes outside a client's family are never attributed to it, so one crash dialog
//! can no longer mark every running client as failed.
//!
//! Note: Roblox's in-game disconnect screen (e.g. "Error Code: 277") is drawn by the engine inside the
//! game surface and is NOT a Win32 window. No amount of window walking sees it; the log-tail option
//! is what catches those.
use std::collections::{HashMap, HashSet};
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, WPARAM};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, Thread32First, Thread32Next, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumThreadWindows, EnumWindows, GetClassNameW, GetWindowThreadProcessId,
    InternalGetWindowText, IsWindowVisible, PostMessageW, WM_CLOSE,
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

/// family pid -> tracked client pid
fn families(tracked: &[u32]) -> HashMap<u32, u32> {
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
    let mut tops: Vec<HWND> = Vec::new();
    unsafe {
        if let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) {
            let mut te = THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
            if Thread32First(snap, &mut te).is_ok() {
                loop {
                    if fam.contains_key(&te.th32OwnerProcessID) {
                        let _ = EnumThreadWindows(te.th32ThreadID, Some(push_hwnd), LPARAM(&mut tops as *mut _ as isize));
                    }
                    if Thread32Next(snap, &mut te).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
        }
        let _ = EnumWindows(Some(push_hwnd), LPARAM(&mut tops as *mut _ as isize));
    }

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
    let t = w.text.to_lowercase();
    if w.top_level && t == "roblox" {
        return false; // the normal game window
    }
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
pub fn has_main_window(ws: &[Win], pid: u32) -> bool {
    ws.iter().any(|w| w.top_level && w.visible && w.pid == pid && w.text == "Roblox")
}

pub fn top_levels_of(ws: &[Win], pid: u32) -> Vec<HWND> {
    ws.iter().filter(|w| w.top_level && w.pid == pid).map(|w| w.hwnd).collect()
}

pub fn close(h: HWND) {
    unsafe {
        let _ = PostMessageW(h, WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}
