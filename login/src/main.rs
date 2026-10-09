#![windows_subsystem = "windows"]
//! ram-login.exe — the sign-in helper. The only binary in the workspace that links tao/wry.
//!
//! Contract with the manager:
//!   * argv: `--data-dir <dir>` — a fresh, per-launch WebView2 profile folder chosen by the manager.
//!   * stdout MUST be a pipe (the manager hands us the write end of an anonymous pipe with an
//!     owner-only DACL). If stdout is a console, file or anything else we refuse to run, so the
//!     cookie can never be printed to a terminal or redirected to disk.
//!   * On success: write `<.ROBLOSECURITY value>\n` to stdout, exit 0.
//!   * Window closed: exit 2. Bad stdout: exit 3. Bad args: exit 4.
use std::time::{Duration, Instant};
use tao::{
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};
use wry::WebViewBuilder;

const EXIT_CANCELLED: i32 = 2;
const EXIT_BAD_STDOUT: i32 = 3;
const EXIT_BAD_ARGS: i32 = 4;

#[cfg(windows)]
fn stdout_is_pipe() -> bool {
    use windows::Win32::Storage::FileSystem::{GetFileType, FILE_TYPE_PIPE};
    use windows::Win32::System::Console::{GetStdHandle, STD_OUTPUT_HANDLE};
    unsafe {
        match GetStdHandle(STD_OUTPUT_HANDLE) {
            Ok(h) if !h.is_invalid() => GetFileType(h) == FILE_TYPE_PIPE,
            _ => false,
        }
    }
}
#[cfg(not(windows))]
fn stdout_is_pipe() -> bool {
    false
}

fn data_dir_arg() -> Option<std::path::PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--data-dir" {
            let p = std::path::PathBuf::from(args.next()?);
            // Only accept a folder inside the user's temp dir (that's where the manager creates it).
            return p.starts_with(std::env::temp_dir()).then_some(p);
        }
    }
    None
}

fn main() {
    if !stdout_is_pipe() {
        std::process::exit(EXIT_BAD_STDOUT);
    }
    let Some(dir) = data_dir_arg() else { std::process::exit(EXIT_BAD_ARGS) };
    std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &dir);

    let el = EventLoop::new();
    let win = WindowBuilder::new()
        .with_title("Sign in to Roblox")
        .with_inner_size(tao::dpi::LogicalSize::new(480.0, 740.0))
        .build(&el)
        .expect("create window");
    let wv = WebViewBuilder::new()
        .with_url("https://www.roblox.com/login")
        .with_incognito(true)
        .build(&win)
        .expect("create webview");

    el.run(move |ev, _, cf| {
        *cf = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(1000));
        match ev {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => std::process::exit(EXIT_CANCELLED),
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                // Signed in once we land on /home. The cookie is HttpOnly, so read it from the
                // cookie manager, not from page JS.
                let Ok(url) = wv.url() else { return };
                if !url.contains("roblox.com/home") {
                    return;
                }
                let Ok(cookies) = wv.cookies_for_url("https://www.roblox.com") else { return };
                if let Some(c) = cookies.iter().find(|c| c.name() == ".ROBLOSECURITY") {
                    use std::io::Write;
                    let mut out = std::io::stdout().lock();
                    let ok = out.write_all(c.value().as_bytes()).and_then(|_| out.write_all(b"\n")).and_then(|_| out.flush());
                    std::process::exit(if ok.is_ok() { 0 } else { EXIT_BAD_STDOUT });
                }
            }
            _ => {}
        }
    })
}
