//! Multiple Roblox clients at once.
//!
//! At start-up a Roblox client creates a named EVENT, "ROBLOX_singletonEvent". If it already exists,
//! the newcomer signals it and the client that's waiting on it closes — that's the "open a second
//! account and the first one crashes" behaviour. (Older clients used a mutex, "ROBLOX_singletonMutex".)
//!
//! The fix multi-instance tools use: create a MUTEX called "ROBLOX_singletonEvent" before any client
//! starts. Windows keeps one namespace for both kinds of object, so the client's CreateEvent for that
//! name fails (the name belongs to a mutex) and the client skips the single-instance handshake.
//!
//! The previous version here created an *event* with that name instead. That's exactly the object
//! Roblox uses, so it changed nothing — worse, it was manual-reset and we kept it alive, so once one
//! client signalled it, it stayed signalled and every later client closed the one before it.
//!
//! This only works if no Roblox client is running when we take the name. If one is, `blocked` is set
//! and launches are held back (see launcher::do_launch) until every Roblox window is closed.
use crossbeam_channel::{bounded, Sender};
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;

pub struct SingletonGuard {
    /// Dropping this ends the holder thread, which closes the handles.
    _stop: Sender<()>,
    /// A Roblox client already owns "ROBLOX_singletonEvent" (as an event), so our mutex couldn't be
    /// created: a new launch would close that client.
    pub blocked: bool,
}

/// Takes the names on a dedicated thread that lives exactly as long as the guard, so mutex ownership
/// is never tied to a pooled thread that might exit (an owned mutex is abandoned when its thread dies).
pub fn acquire() -> SingletonGuard {
    let (res_tx, res_rx) = bounded::<bool>(1);
    let (stop_tx, stop_rx) = bounded::<()>(0);
    let spawned = std::thread::Builder::new()
        .name("roblox-singleton".into())
        .stack_size(64 * 1024)
        .spawn(move || {
            let mut held: Vec<HANDLE> = Vec::new();
            let mut blocked = false;
            unsafe {
                match CreateMutexW(None, true, w!("ROBLOX_singletonEvent")) {
                    Ok(h) => held.push(h),
                    // ERROR_INVALID_HANDLE: the name exists as a different object type, i.e. a running
                    // client's event.
                    Err(_) => blocked = true,
                }
                if let Ok(h) = CreateMutexW(None, true, w!("ROBLOX_singletonMutex")) {
                    held.push(h);
                }
            }
            let _ = res_tx.send(blocked);
            let _ = stop_rx.recv(); // returns when the guard (sender) is dropped
            for h in held {
                unsafe {
                    let _ = CloseHandle(h);
                }
            }
        });
    let blocked = spawned.is_err() || res_rx.recv().unwrap_or(true);
    SingletonGuard { _stop: stop_tx, blocked }
}
