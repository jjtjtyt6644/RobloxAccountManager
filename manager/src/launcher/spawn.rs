use windows::core::PCWSTR;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Hands the roblox-player:// URI to the installed Roblox protocol handler via the shell.
///
/// Blocking, and must NOT run on a tokio worker: ShellExecute may delegate to shell extensions and
/// requires COM to be initialised (STA) on the calling thread, which tokio workers never do. Call it
/// through `tokio::task::spawn_blocking`; we initialise COM for the duration of the call.
pub fn open(uri: &str) -> Result<(), String> {
    let op: Vec<u16> = "open\0".encode_utf16().collect();
    let target: Vec<u16> = uri.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let com = CoInitializeEx(None, COINIT(COINIT_APARTMENTTHREADED.0 | COINIT_DISABLE_OLE1DDE.0));
        let r = ShellExecuteW(None, PCWSTR(op.as_ptr()), PCWSTR(target.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        if com.is_ok() {
            CoUninitialize();
        }
        if (r.0 as isize) > 32 {
            Ok(())
        } else {
            Err(format!("ShellExecute failed ({})", r.0 as isize))
        }
    }
}
