//! Updates: the manager itself (GitHub releases) and the Roblox player (official Roblox CDN).
//!
//! Nothing is installed without the user asking: Settings → Updates has "Check for updates" (and an
//! opt-in "install automatically"), and the Roblox player is only updated when its button is pressed.
//! Both downloads are HTTPS-only, from fixed hosts, size-capped and checked to be Windows executables.
pub mod app_update;
pub mod roblox;

use std::io::{Read, Write};
use std::path::Path;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPO: &str = "jjtjtyt6644/RobloxAccountManager";

/// State of one background job, shown as a single line under its buttons.
#[derive(Clone, Default, PartialEq)]
pub enum Job {
    #[default]
    Idle,
    Working(String),
    Done(String),
    Failed(String),
}

impl Job {
    pub fn busy(&self) -> bool {
        matches!(self, Job::Working(_))
    }
}

#[derive(Clone, Default)]
pub struct Updates {
    pub app: Job,
    pub app_latest: Option<app_update::Release>,
    /// The new exe is in place; restarting the manager finishes the update.
    pub app_installed: bool,
    /// 0.0–1.0 while the new version downloads (the updater window's progress bar).
    pub app_progress: Option<f32>,
    /// Installed and the user asked for it: the UI closes and reopens the manager now.
    pub restart_now: bool,
    pub roblox: Job,
    pub roblox_installed: Option<String>,
    pub roblox_latest: Option<roblox::Latest>,
}

/// Streams `url` to `dest`, refusing anything over `max` bytes or (when known) not exactly `expect`
/// bytes, and anything that isn't a Windows executable. `progress(done, total)` is called per chunk.
pub async fn download(
    http: &reqwest::Client,
    url: &str,
    dest: &Path,
    expect: Option<u64>,
    max: u64,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<(), String> {
    let mut r = http
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("download failed: {e}"))?;
    let total = r.content_length().or(expect);
    if total.map_or(false, |t| t > max) {
        return Err("download is larger than expected — refusing it".into());
    }
    let mut f = std::fs::File::create(dest).map_err(|e| format!("can't write {}: {e}", dest.display()))?;
    let mut n = 0u64;
    while let Some(c) = r.chunk().await.map_err(|e| format!("download interrupted: {e}"))? {
        n += c.len() as u64;
        if n > max {
            return Err("download is larger than expected — refusing it".into());
        }
        f.write_all(&c).map_err(|e| e.to_string())?;
        progress(n, total);
    }
    f.flush().map_err(|e| e.to_string())?;
    drop(f);
    if expect.map_or(false, |e| e != n) {
        let _ = std::fs::remove_file(dest);
        return Err("download was incomplete — try again".into());
    }
    let mut head = [0u8; 2];
    let is_exe = std::fs::File::open(dest).and_then(|mut f| f.read_exact(&mut head)).is_ok() && &head == b"MZ";
    if !is_exe {
        let _ = std::fs::remove_file(dest);
        return Err("downloaded file isn't a Windows program — refusing it".into());
    }
    Ok(())
}

/// "Downloading… 42%" (or MB when the size is unknown), only when the shown text would change.
pub fn progress_text(done: u64, total: Option<u64>) -> String {
    match total {
        Some(t) if t > 0 => format!("Downloading… {}%", done * 100 / t),
        _ => format!("Downloading… {:.1} MB", done as f64 / 1048576.0),
    }
}

/// Opens a web link in the default browser (off the UI thread; ShellExecute may block briefly).
pub fn open_url(url: String) {
    std::thread::spawn(move || {
        let _ = crate::launcher::spawn::open(&url);
    });
}
