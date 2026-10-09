//! Self-update from the GitHub releases of `REPO`.
//!
//! Install = download the release's roblox_account_manager.exe next to ours as `.new`, rename the
//! running exe to `.old` (Windows allows renaming a running exe, not overwriting it), move `.new` into
//! place. The next start runs the new version and deletes the `.old` file. ram-login.exe is replaced
//! the same way when the release includes it.
use super::{download, progress_text, Job, REPO, VERSION};
use crate::launcher::Engine;
use std::path::{Path, PathBuf};

const MAX_EXE: u64 = 64 * 1024 * 1024;

#[derive(Clone)]
pub struct Asset {
    pub url: String,
    pub size: u64,
}

#[derive(Clone)]
pub struct Release {
    pub tag: String,
    pub version: [u32; 3],
    pub notes: String,
    pub page: String,
    pub published: String,
    pub exe: Asset,
    pub login: Option<Asset>,
}

impl Release {
    pub fn is_newer(&self) -> bool {
        parse_version(VERSION).map_or(false, |cur| self.version > cur)
    }
}

/// "v1.2.3", "1.2", "v1.2.3-beta" → [1,2,3] / [1,2,0] / [1,2,3].
pub fn parse_version(s: &str) -> Option<[u32; 3]> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+', ' ']).next()?;
    let mut v = [0u32; 3];
    for (i, part) in core.split('.').take(3).enumerate() {
        v[i] = part.parse().ok()?;
    }
    Some(v)
}

async fn fetch_latest(http: &reqwest::Client) -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = http
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| {
            if e.status().map_or(false, |s| s.as_u16() == 404) {
                "no releases published yet".to_string()
            } else {
                format!("couldn't reach GitHub: {e}")
            }
        })?
        .bytes()
        .await
        .map_err(|e| format!("couldn't read the reply from GitHub: {e}"))?;
    let j: serde_json::Value =
        serde_json::from_slice(&body).map_err(|e| format!("unexpected reply from GitHub: {e}"))?;
    let tag = j["tag_name"].as_str().unwrap_or_default().to_string();
    let version = parse_version(&tag).ok_or_else(|| format!("release tag \"{tag}\" isn't a version number"))?;
    // Only accept files from this repository's own release downloads.
    let prefix = format!("https://github.com/{REPO}/releases/download/");
    let assets: Vec<(String, Asset)> = j["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    let name = x["name"].as_str()?.to_ascii_lowercase();
                    let url = x["browser_download_url"].as_str()?.to_string();
                    let size = x["size"].as_u64()?;
                    (url.starts_with(&prefix) && name.ends_with(".exe")).then_some((name, Asset { url, size }))
                })
                .collect()
        })
        .unwrap_or_default();
    let pick = |n: &str| assets.iter().find(|(name, _)| name == n).map(|(_, a)| a.clone());
    let exe = pick("roblox_account_manager.exe").ok_or("the latest release has no roblox_account_manager.exe")?;
    Ok(Release {
        tag,
        version,
        notes: j["body"].as_str().unwrap_or_default().replace("\r\n", "\n").trim().to_string(),
        page: j["html_url"].as_str().unwrap_or_default().to_string(),
        published: j["published_at"].as_str().unwrap_or_default().chars().take(10).collect(),
        exe,
        login: pick("ram-login.exe"),
    })
}

fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let mut s = exe.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Swap `new` into `target`, keeping the old one as `target.old`. Rolls back on failure.
fn swap_in(target: &Path, new: &Path) -> Result<(), String> {
    let old = sibling(target, ".old");
    let _ = std::fs::remove_file(&old);
    let had_old = target.exists();
    if had_old {
        std::fs::rename(target, &old).map_err(|e| format!("can't replace {}: {e}", target.display()))?;
    }
    if let Err(e) = std::fs::rename(new, target) {
        if had_old {
            let _ = std::fs::rename(&old, target);
        }
        return Err(format!("can't put the new version in place: {e}"));
    }
    Ok(())
}

/// Start-up: delete the `.old` files a previous update left behind (they were running then).
pub fn cleanup_old() {
    let Ok(exe) = std::env::current_exe() else { return };
    let _ = std::fs::remove_file(sibling(&exe, ".old"));
    if let Some(dir) = exe.parent() {
        let _ = std::fs::remove_file(dir.join("ram-login.exe.old"));
    }
}

impl Engine {
    fn set_app_job(&self, j: Job) {
        self.updates.lock().unwrap().app = j;
        self.ui.ping();
    }

    /// `auto_install`: install straight away if a newer version is found (start-up, when enabled).
    pub fn check_app_update(&self, auto_install: bool) {
        if self.updates.lock().unwrap().app.busy() {
            return;
        }
        self.set_app_job(Job::Working("Checking GitHub for a new version…".into()));
        let e = self.clone();
        self.rt.spawn(async move {
            match fetch_latest(&e.http).await {
                Ok(r) => {
                    let newer = r.is_newer();
                    let msg = if newer {
                        format!("Version {} is available (you have {VERSION}).", r.tag)
                    } else {
                        format!("You're on the latest version ({VERSION}).")
                    };
                    e.updates.lock().unwrap().app_latest = Some(r);
                    e.set_app_job(Job::Done(msg));
                    if newer && auto_install {
                        e.install_app_update(true);
                    }
                }
                Err(er) => e.set_app_job(Job::Failed(er)),
            }
        });
    }

    async fn fetch_asset(&self, a: &Asset, dest: &Path) -> Result<(), String> {
        let mut shown = String::new();
        download(&self.http, &a.url, dest, Some(a.size), MAX_EXE, |d, t| {
            let s = progress_text(d, t);
            if s != shown {
                shown = s.clone();
                self.updates.lock().unwrap().app_progress = t.filter(|t| *t > 0).map(|t| d as f32 / t as f32);
                self.set_app_job(Job::Working(s));
            }
        })
        .await
    }

    /// `restart`: reopen the manager on the new version as soon as it's installed (the updater
    /// window). Running Roblox windows are picked up again by the new copy (Engine::adopt_running).
    pub fn install_app_update(&self, restart: bool) {
        let rel = {
            let u = self.updates.lock().unwrap();
            if u.app.busy() || u.app_installed {
                return;
            }
            match &u.app_latest {
                Some(r) if r.is_newer() => r.clone(),
                _ => return,
            }
        };
        self.set_app_job(Job::Working("Downloading…".into()));
        let e = self.clone();
        self.rt.spawn(async move {
            let res: Result<(), String> = async {
                let exe = std::env::current_exe().map_err(|er| er.to_string())?;
                let dir = exe.parent().ok_or("can't find the manager's folder")?.to_path_buf();
                let new_exe = sibling(&exe, ".new");
                e.fetch_asset(&rel.exe, &new_exe).await?;
                let new_login = match &rel.login {
                    Some(a) => {
                        let p = dir.join("ram-login.exe.new");
                        e.fetch_asset(a, &p).await?;
                        Some(p)
                    }
                    None => None,
                };
                e.set_app_job(Job::Working("Installing…".into()));
                tokio::task::spawn_blocking(move || {
                    swap_in(&exe, &new_exe)?;
                    if let Some(p) = new_login {
                        swap_in(&dir.join("ram-login.exe"), &p)?;
                    }
                    Ok::<(), String>(())
                })
                .await
                .map_err(|er| er.to_string())?
            }
            .await;
            match res {
                Ok(()) => {
                    {
                        let mut u = e.updates.lock().unwrap();
                        u.app_installed = true;
                        u.app_progress = None;
                        u.restart_now = restart;
                    }
                    e.log(0, "update", &format!("installed {}", rel.tag));
                    e.ui.ctx.request_repaint(); // even if minimized: the UI thread performs the restart
                    e.set_app_job(Job::Done(if restart {
                        format!("Version {} installed — reopening…", rel.tag)
                    } else {
                        format!("Version {} is installed. Restart the manager to finish.", rel.tag)
                    }));
                }
                Err(er) => {
                    e.updates.lock().unwrap().app_progress = None;
                    e.log(0, "update_failed", &er);
                    e.set_app_job(Job::Failed(er));
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::parse_version;
    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.0.0"), Some([1, 0, 0]));
        assert_eq!(parse_version("1.2"), Some([1, 2, 0]));
        assert_eq!(parse_version("v2.10.3-beta"), Some([2, 10, 3]));
        assert!(parse_version("latest").is_none());
        assert!(parse_version("v1.10.0") > parse_version("v1.9.9"));
    }
}
