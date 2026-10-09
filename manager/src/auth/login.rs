//! Runs ram-login.exe and receives the cookie over an ANONYMOUS pipe.
//!
//! Why not `Stdio::piped()`: on Windows, Rust std implements it as a *named* pipe
//! (`\\.\pipe\__rust_anonymous_pipe1__.<pid>.<n>`) with the default DACL, because std needs
//! overlapped I/O. We don't need overlapped I/O for one small blocking read, so we use a real
//! anonymous pipe from `CreatePipe`, and we also give it an explicit, protected, owner-only DACL:
//!
//!   D:P(A;;GA;;;<current user SID>)
//!
//! * No name -> nothing to guess, squat on, or connect to.
//! * Protected DACL with a single ACE -> even an attempt to open it by handle duplication requires
//!   being the same user; SYSTEM/Administrators/Everyone get no ACE.
//! * Neither end is created inheritable. std duplicates the write end as inheritable only for the
//!   duration of CreateProcess (under its spawn lock), so no other child can inherit it.
//! * The helper itself refuses to run if its stdout is not a pipe (see login/src/main.rs).
use std::fs::File;
use std::io::Read;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, LocalFree, BOOL, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
};
use windows::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use zeroize::{Zeroize, Zeroizing};

pub const HELPER_EXE: &str = "ram-login.exe";
const MAX_COOKIE: usize = 16 * 1024;
const SDDL_REVISION_1: u32 = 1;

pub enum LoginErr {
    HelperMissing(PathBuf),
    Cancelled,
    Failed(String),
}

/// ram-login.exe built into this exe (see build.rs). Empty when the build didn't include it.
static EMBEDDED_HELPER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ram-login.bin"));

/// The built-in helper is unpacked to %APPDATA%\RobloxAccountManager\bin\ram-login.exe (rewritten
/// whenever it differs, so an updated manager always runs its matching helper). Without a built-in
/// copy, a ram-login.exe next to the manager is used.
pub fn helper_path() -> Option<PathBuf> {
    if !EMBEDDED_HELPER.is_empty() {
        let dir = crate::storage::db::Db::dir().join("bin");
        let path = dir.join(HELPER_EXE);
        let current = std::fs::read(&path).map_or(false, |b| b == EMBEDDED_HELPER);
        if current || (std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&path, EMBEDDED_HELPER).is_ok()) {
            return Some(path);
        }
    }
    std::env::current_exe().ok().map(|p| p.with_file_name(HELPER_EXE))
}

/// Fresh WebView2 profile folder per sign-in, so nothing from a previous sign-in is reused.
fn new_data_dir() -> PathBuf {
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let r = RandomState::new().hash_one(nanos);
    std::env::temp_dir().join(format!("ram-login-{r:016x}"))
}

/// String SID of the user this process runs as, e.g. "S-1-5-21-...".
unsafe fn current_user_sid() -> Result<String, String> {
    let mut tok = HANDLE::default();
    OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok).map_err(|e| e.to_string())?;
    let mut len = 0u32;
    let _ = GetTokenInformation(tok, TokenUser, None, 0, &mut len); // sizing call, expected to "fail"
    let mut buf = vec![0u64; (len as usize + 7) / 8 + 1]; // u64 => suitably aligned for TOKEN_USER
    let r = GetTokenInformation(tok, TokenUser, Some(buf.as_mut_ptr() as *mut _), len, &mut len);
    let _ = CloseHandle(tok);
    r.map_err(|e| e.to_string())?;
    let tu = &*(buf.as_ptr() as *const TOKEN_USER);
    let mut s = PWSTR::null();
    ConvertSidToStringSidW(tu.User.Sid, &mut s).map_err(|e| e.to_string())?;
    let out = s.to_string().map_err(|e| e.to_string());
    let _ = LocalFree(HLOCAL(s.0 as *mut _));
    out
}

/// (read end, write end). Neither handle is inheritable.
fn owner_only_anonymous_pipe() -> Result<(OwnedHandle, OwnedHandle), String> {
    unsafe {
        let sid = current_user_sid()?;
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})").encode_utf16().chain(Some(0)).collect();
        let mut sd = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
        ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), SDDL_REVISION_1, &mut sd, None)
            .map_err(|e| e.to_string())?;
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: BOOL(0),
        };
        let mut r = HANDLE::default();
        let mut w = HANDLE::default();
        let res = CreatePipe(&mut r, &mut w, Some(&sa as *const SECURITY_ATTRIBUTES), 0);
        let _ = LocalFree(HLOCAL(sd.0));
        res.map_err(|e| e.to_string())?;
        Ok((OwnedHandle::from_raw_handle(r.0), OwnedHandle::from_raw_handle(w.0)))
    }
}

fn remove_dir_retry(dir: &Path) {
    // WebView2 browser processes can hold files for a moment after the helper exits.
    for _ in 0..10 {
        if !dir.exists() || std::fs::remove_dir_all(dir).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Blocking. Run on a background thread.
pub fn run() -> Result<Zeroizing<String>, LoginErr> {
    let exe = helper_path().ok_or_else(|| LoginErr::Failed("cannot locate own exe".into()))?;
    if !exe.is_file() {
        return Err(LoginErr::HelperMissing(exe));
    }
    let dir = new_data_dir();
    let (read, write) = owner_only_anonymous_pipe().map_err(LoginErr::Failed)?;

    let mut cmd = Command::new(&exe);
    cmd.arg("--data-dir").arg(&dir).stdin(Stdio::null()).stdout(Stdio::from(write)).stderr(Stdio::null());
    let spawned = cmd.spawn();
    drop(cmd); // closes OUR copy of the write end, so the read below sees EOF when the helper exits
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            remove_dir_retry(&dir);
            return Err(LoginErr::Failed(format!("could not start sign-in helper: {e}")));
        }
    };

    // Read until newline (don't depend on EOF in case a webview subprocess still holds a handle).
    // Capacity is reserved up front so the buffer never reallocates (no stray plaintext copies).
    let mut f = File::from(read);
    let mut buf: Zeroizing<Vec<u8>> = Zeroizing::new(Vec::with_capacity(MAX_COOKIE + 1024));
    let mut chunk = [0u8; 1024];
    loop {
        match f.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.contains(&b'\n') || buf.len() >= MAX_COOKIE {
                    break;
                }
            }
        }
    }
    chunk.zeroize();
    drop(f);
    let status = child.wait();
    remove_dir_retry(&dir);

    let code = status.ok().and_then(|s| s.code());
    let line_end = buf.iter().position(|b| *b == b'\n');
    match (code, line_end) {
        (Some(0), Some(end)) => {
            let v = std::str::from_utf8(&buf[..end]).map(|s| s.trim()).unwrap_or("");
            if v.is_empty() || !v.bytes().all(|b| b.is_ascii_graphic()) {
                return Err(LoginErr::Failed("sign-in helper returned an unexpected value".into()));
            }
            Ok(Zeroizing::new(v.to_string()))
        }
        (Some(2), _) => Err(LoginErr::Cancelled),
        (Some(3), _) => Err(LoginErr::Failed("sign-in helper refused: its output was not a private pipe".into())),
        (c, _) => Err(LoginErr::Failed(format!("sign-in helper exited unexpectedly ({c:?})"))),
    }
}
