//! Roblox player version check and update.
//!
//! The installed version is the newest `%LOCALAPPDATA%\Roblox\Versions\version-…` folder that holds
//! RobloxPlayerBeta.exe; the latest comes from Roblox's own client-version endpoint. Updating
//! downloads Roblox's official installer for that exact version from setup.rbxcdn.com and runs it —
//! the same thing the roblox.com "Download" button does — so we never assemble Roblox files ourselves.
use super::{download, progress_text, Job};
use crate::launcher::{fastflags, Engine};
use std::time::Duration;

const MAX_INSTALLER: u64 = 64 * 1024 * 1024;

#[derive(Clone)]
pub struct Latest {
    /// Human version, e.g. "0.742.0.7421053".
    pub version: String,
    /// Folder/build id, e.g. "version-cec3ad5889b447cf".
    pub upload: String,
}

pub fn installed() -> Option<String> {
    Some(fastflags::current_version()?.file_name()?.to_string_lossy().into_owned())
}

async fn fetch_latest(http: &reqwest::Client) -> Result<Latest, String> {
    let body = http
        .get("https://clientsettingscdn.roblox.com/v2/client-version/WindowsPlayer")
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("couldn't reach Roblox: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("couldn't read the reply from Roblox: {e}"))?;
    let j: serde_json::Value =
        serde_json::from_slice(&body).map_err(|e| format!("unexpected reply from Roblox: {e}"))?;
    let upload = j["clientVersionUpload"].as_str().unwrap_or_default().to_string();
    // It goes into a download URL: only "version-" + hex is accepted.
    let ok = upload.strip_prefix("version-").map_or(false, |h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()));
    if !ok {
        return Err("unexpected version id from Roblox".into());
    }
    Ok(Latest { version: j["version"].as_str().unwrap_or("?").to_string(), upload })
}

impl Engine {
    fn set_roblox_job(&self, j: Job) {
        self.updates.lock().unwrap().roblox = j;
        self.ui.ping();
    }

    pub fn check_roblox_update(&self) {
        if self.updates.lock().unwrap().roblox.busy() {
            return;
        }
        self.set_roblox_job(Job::Working("Checking Roblox's latest version…".into()));
        let e = self.clone();
        self.rt.spawn(async move {
            let have = tokio::task::spawn_blocking(installed).await.ok().flatten();
            let res = fetch_latest(&e.http).await;
            let job = match &res {
                Ok(l) => match &have {
                    Some(h) if *h == l.upload => Job::Done(format!("Roblox is up to date ({}).", l.version)),
                    Some(_) => Job::Done(format!("A Roblox update is available ({}).", l.version)),
                    None => Job::Done("Roblox isn't installed for this Windows user.".into()),
                },
                Err(er) => Job::Failed(er.clone()),
            };
            {
                let mut u = e.updates.lock().unwrap();
                u.roblox_installed = have;
                if let Ok(l) = res {
                    u.roblox_latest = Some(l);
                }
            }
            e.set_roblox_job(job);
        });
    }

    /// Downloads and runs Roblox's official installer. The caller makes sure no client is running
    /// (the installer closes them).
    pub fn update_roblox(&self) {
        if self.updates.lock().unwrap().roblox.busy() {
            return;
        }
        self.set_roblox_job(Job::Working("Checking Roblox's latest version…".into()));
        let e = self.clone();
        self.rt.spawn(async move {
            let res: Result<String, String> = async {
                let latest = fetch_latest(&e.http).await?;
                let url = format!("https://setup.rbxcdn.com/{}-RobloxPlayerInstaller.exe", latest.upload);
                let dest = std::env::temp_dir().join("RAM-RobloxPlayerInstaller.exe");
                let mut shown = String::new();
                download(&e.http, &url, &dest, None, MAX_INSTALLER, |d, t| {
                    let s = progress_text(d, t);
                    if s != shown {
                        shown = s.clone();
                        e.set_roblox_job(Job::Working(s));
                    }
                })
                .await?;
                e.set_roblox_job(Job::Working("Roblox installer is running — follow its window…".into()));
                let run = dest.clone();
                let status = tokio::task::spawn_blocking(move || std::process::Command::new(&run).status())
                    .await
                    .map_err(|er| er.to_string())?
                    .map_err(|er| format!("couldn't start the Roblox installer: {er}"))?;
                // The installer can hand off to a child process and return before the files are in
                // place, so wait for the new version folder rather than trusting the exit code.
                e.set_roblox_job(Job::Working("Finishing the Roblox install…".into()));
                for _ in 0..if status.success() { 180 } else { 5 } {
                    if tokio::task::spawn_blocking(installed).await.ok().flatten().as_deref() == Some(&latest.upload) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                let _ = std::fs::remove_file(&dest);
                let have = tokio::task::spawn_blocking(installed).await.ok().flatten();
                let ok = have.as_deref() == Some(&latest.upload);
                if !ok && !status.success() {
                    return Err(format!("the Roblox installer stopped with code {}", status.code().unwrap_or(-1)));
                }
                {
                    let mut u = e.updates.lock().unwrap();
                    u.roblox_installed = have;
                    u.roblox_latest = Some(latest.clone());
                }
                if ok {
                    Ok(format!("Roblox updated to {}.", latest.version))
                } else {
                    Err("the installer finished but the new version wasn't found — try again".into())
                }
            }
            .await;
            match res {
                Ok(m) => {
                    e.log(0, "roblox_update", &m);
                    e.set_roblox_job(Job::Done(m));
                }
                Err(er) => {
                    e.log(0, "update_failed", &er);
                    e.set_roblox_job(Job::Failed(er));
                }
            }
        });
    }
}
