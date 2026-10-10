use crate::storage::crypto;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

pub type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

#[derive(Clone, Debug)]
pub struct Account {
    pub id: i64,
    pub label: String,
    pub username: String,
    pub place_id: String,
    pub preset_id: i64,
    pub group_tag: String,
    pub last_job_id: Option<String>,
    pub status: String,
    /// Game name, when the game was picked from search ("" otherwise).
    pub place_name: String,
    /// Open this account automatically when the manager starts.
    pub auto_start: bool,
}

/// A performance profile's settings. `#[serde(default)]` so profiles saved by older versions load.
#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Gfx {
    /// Frame-rate cap via Roblox's own setting: 0 = don't change, 240+ = uncapped.
    pub fps: u32,
    /// Texture quality: 0 = full (not changed), 1 = high, 2 = medium, 3+ = lowest.
    pub skip_mips: u32,
    /// Graphics quality 1–10 via Roblox's own setting; 0 = don't change.
    pub graphics_quality: u32,
    /// No anti-aliasing (allow-listed flag).
    pub msaa_off: bool,
    /// No grass (allow-listed flags).
    pub grass_off: bool,
    /// Volume to 0 for windows using this profile.
    pub mute: bool,
    // Kept so old profiles still load; Roblox denies these flags now, so they're not written.
    pub post_fx_off: bool,
    pub shadows_off: bool,
    pub fb_cap: u32,
}
impl Default for Gfx {
    fn default() -> Self {
        Gfx {
            fps: 0,
            skip_mips: 0,
            graphics_quality: 0,
            msaa_off: false,
            grass_off: false,
            mute: false,
            post_fx_off: false,
            shadows_off: false,
            fb_cap: 0,
        }
    }
}

/// Light profile for alts: 30 fps, low quality, medium textures, no anti-aliasing or grass, muted.
fn alt_low() -> Gfx {
    Gfx { fps: 30, skip_mips: 2, graphics_quality: 3, msaa_off: true, grass_off: true, mute: true, ..Gfx::default() }
}
/// Lightest profile for AFK alts.
fn alt_minimal() -> Gfx {
    Gfx { fps: 15, skip_mips: 3, graphics_quality: 1, msaa_off: true, grass_off: true, mute: true, ..Gfx::default() }
}

#[derive(Clone, Default)]
pub struct Preset {
    pub id: i64,
    pub name: String,
    pub gfx: Gfx,
    pub priority: i32, // 0 Normal, 1 BelowNormal, 2 Idle
    pub trim_interval_sec: i64,
}

/// Memory saver level for Roblox clients (runtime::trim). The window in the foreground is never trimmed.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, Debug)]
pub enum TrimMode {
    #[default]
    Off,
    /// Every 15 s: trim background clients above the target.
    Balanced,
    /// Every 5 s: same as Balanced.
    Strong,
    /// Hard working-set ceiling at the target for background clients.
    Max,
}

/// CPU saver level for background Roblox clients (runtime::trim). The foreground window is never throttled.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, Debug)]
pub enum CpuMode {
    #[default]
    Off,
    /// Windows Efficiency mode (EcoQoS) + below-normal priority.
    Efficiency,
    /// Efficiency + idle priority + at most 2 CPU cores.
    Strong,
}

/// `#[serde(default)]`: settings saved by an older version (missing new fields) still load instead of
/// silently resetting everything to defaults.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub auto_reconnect: bool,
    pub max_attempts: u32,
    pub delay_secs: u64,
    /// Read each client's Roblox log to notice disconnects, kicks, server shutdowns and the like
    /// (they're drawn inside the game, not as windows), plus frozen windows and failed joins.
    pub detect_disconnects: bool,
    /// Also count "left the game, back on the Roblox home screen" as something to recover from.
    pub reopen_if_left: bool,
    /// Title each Roblox window after its account ("Roblox — Main").
    pub title_windows: bool,
    /// One Roblox window per account: a second window of an account that's already open is closed
    /// straight away and the first one keeps playing.
    pub one_window_per_account: bool,
    pub retry_on_auth_fail: bool, // applies to non-401 ticket failures; 401 always => needs_relogin
    pub trim_mode: TrimMode,
    /// Per background client, in MB.
    pub trim_target_mb: u32,
    pub cpu_saver: CpuMode,
    /// Savers also apply to the window you're playing in (default: background windows only).
    pub saver_all_windows: bool,
    /// Manager itself: Efficiency mode while unfocused, no redraws and a memory trim while minimized.
    pub manager_light: bool,
    /// Pre-0.3 on/off switch. Read once to migrate to `trim_mode`, never written back.
    #[serde(skip_serializing)]
    pub trim_enabled: bool,
    pub kill_on_exit: bool,
    /// Hold Roblox's single-instance lock so several clients can run.
    pub multi_instance: bool,
    /// Rejoin the previous server on reconnect. Only honoured once a real launch URI has been captured
    /// and verified (see launcher::uri_probe); best-effort even then.
    pub same_server_rejoin: bool,
    /// Look for a new manager version (and Roblox version) a few seconds after start.
    pub check_updates_on_start: bool,
    /// Install a new manager version found by that check without asking (applies on next start).
    pub auto_install_updates: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Settings {
            auto_reconnect: true,
            max_attempts: 3,
            delay_secs: 5,
            detect_disconnects: true,
            reopen_if_left: true,
            title_windows: true,
            one_window_per_account: true,
            retry_on_auth_fail: false,
            trim_mode: TrimMode::Off,
            trim_target_mb: 100,
            cpu_saver: CpuMode::Off,
            saver_all_windows: false,
            manager_light: true,
            trim_enabled: false,
            kill_on_exit: false,
            multi_instance: true,
            same_server_rejoin: false,
            check_updates_on_start: true,
            auto_install_updates: false,
        }
    }
}

#[derive(Clone)]
pub struct Ev {
    pub account_id: i64,
    pub ts: i64,
    pub label: String,
    pub event: String,
    pub detail: String,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS accounts(
  id INTEGER PRIMARY KEY, label TEXT NOT NULL, username TEXT NOT NULL,
  encrypted_cookie BLOB NOT NULL, place_id TEXT NOT NULL DEFAULT '',
  preset_id INTEGER NOT NULL DEFAULT 2, group_tag TEXT NOT NULL DEFAULT '',
  last_job_id TEXT, status TEXT NOT NULL DEFAULT 'idle',
  created_at INTEGER NOT NULL, last_used INTEGER);
CREATE TABLE IF NOT EXISTS presets(
  id INTEGER PRIMARY KEY, name TEXT NOT NULL, flags_json TEXT NOT NULL,
  priority INTEGER NOT NULL DEFAULT 1, trim_interval_sec INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS live_clients(
  account_id INTEGER PRIMARY KEY, pid INTEGER NOT NULL, start_time INTEGER NOT NULL,
  launch_ts INTEGER NOT NULL, live_since INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS launch_events(
  id INTEGER PRIMARY KEY, account_id INTEGER NOT NULL, ts INTEGER NOT NULL,
  event TEXT NOT NULL, detail TEXT NOT NULL);
";
const COLS: &str = "id,label,username,place_id,preset_id,group_tag,last_job_id,status,COALESCE(place_name,''),auto_start";

fn row_acc(r: &Row) -> rusqlite::Result<Account> {
    Ok(Account {
        id: r.get(0)?,
        label: r.get(1)?,
        username: r.get(2)?,
        place_id: r.get(3)?,
        preset_id: r.get(4)?,
        group_tag: r.get(5)?,
        last_job_id: r.get(6)?,
        status: r.get(7)?,
        place_name: r.get(8)?,
        auto_start: r.get::<_, i64>(9)? != 0,
    })
}

#[derive(Clone)]
pub struct Db(pub Arc<Mutex<Connection>>);

impl Db {
    pub fn dir() -> PathBuf {
        PathBuf::from(std::env::var("APPDATA").unwrap_or_else(|_| ".".into())).join("RobloxAccountManager")
    }

    pub fn open() -> Res<Self> {
        let dir = Self::dir();
        std::fs::create_dir_all(&dir)?;
        let c = Connection::open(dir.join("accounts.db"))?;
        // The DB is tiny; SQLite's default page cache (~2 MB) is far more than it needs.
        c.execute_batch("PRAGMA cache_size=-256; PRAGMA temp_store=MEMORY;")?;
        c.execute_batch(SCHEMA)?;
        // Added in 1.3.0: Roblox user id, used to recognise clients the manager didn't start.
        let has_uid: bool = c
            .prepare("SELECT 1 FROM pragma_table_info('accounts') WHERE name='user_id'")?
            .exists([])?;
        if !has_uid {
            c.execute_batch("ALTER TABLE accounts ADD COLUMN user_id INTEGER;")?;
        }
        // Added in 1.5.0: the game's name when it was picked from search.
        let has_pname: bool = c.prepare("SELECT 1 FROM pragma_table_info('accounts') WHERE name='place_name'")?.exists([])?;
        if !has_pname {
            c.execute_batch("ALTER TABLE accounts ADD COLUMN place_name TEXT;")?;
        }
        // Added in 1.6.0: open this account when the manager starts.
        let has_auto: bool = c.prepare("SELECT 1 FROM pragma_table_info('accounts') WHERE name='auto_start'")?.exists([])?;
        if !has_auto {
            c.execute_batch("ALTER TABLE accounts ADD COLUMN auto_start INTEGER NOT NULL DEFAULT 0;")?;
        }
        let n: i64 = c.query_row("SELECT COUNT(*) FROM presets", [], |r| r.get(0))?;
        if n == 0 {
            let seeds = [
                ("Main", Gfx::default(), 0),
                ("Alt-Low", alt_low(), 1),
                ("Alt-Minimal", alt_minimal(), 1),
            ];
            for (name, g, pr) in seeds {
                c.execute(
                    "INSERT INTO presets(name,flags_json,priority,trim_interval_sec) VALUES(?1,?2,?3,0)",
                    params![name, serde_json::to_string(&g)?, pr],
                )?;
            }
        }
        // 1.7.0: the seeded profiles used flags Roblox now denies. If they're still exactly as seeded,
        // move them to settings that work (user-edited profiles are left alone).
        for (name, old, new) in [
            ("Main", r#""fps":60,"skip_mips":0,"post_fx_off":false,"shadows_off":false,"fb_cap":0"#, Gfx::default()),
            ("Alt-Low", r#""fps":15,"skip_mips":2,"post_fx_off":true,"shadows_off":true,"fb_cap":0"#, alt_low()),
            ("Alt-Minimal", r#""fps":10,"skip_mips":4,"post_fx_off":true,"shadows_off":true,"fb_cap":0"#, alt_minimal()),
        ] {
            let json = format!("{{{old}}}");
            c.execute(
                "UPDATE presets SET flags_json=?1 WHERE name=?2 AND flags_json=?3",
                params![serde_json::to_string(&new)?, name, json],
            )?;
        }
        // Any other profile saved before 1.7.0 (no "graphics_quality" yet): its shadows/effects
        // switches no longer work, so carry the intent over to the quality setting Roblox accepts.
        let old: Vec<(i64, String)> = c
            .prepare("SELECT id,flags_json FROM presets WHERE flags_json NOT LIKE '%graphics_quality%'")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        for (id, json) in old {
            let mut g: Gfx = serde_json::from_str(&json).unwrap_or_default();
            if g.shadows_off || g.post_fx_off {
                g.graphics_quality = if g.shadows_off && g.post_fx_off { 1 } else { 3 };
                g.msaa_off = true;
            }
            c.execute("UPDATE presets SET flags_json=?1 WHERE id=?2", params![serde_json::to_string(&g)?, id])?;
        }
        Ok(Db(Arc::new(Mutex::new(c))))
    }

    fn c(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.0.lock().unwrap()
    }

    // ---------- accounts ----------
    pub fn upsert_account(&self, username: &str, cookie: &str) -> Res<i64> {
        let blob = crypto::protect(cookie.as_bytes())?;
        let c = self.c();
        let existing: Option<i64> = c
            .query_row("SELECT id FROM accounts WHERE username=?1 COLLATE NOCASE", [username], |r| r.get(0))
            .optional()?;
        if let Some(id) = existing {
            c.execute("UPDATE accounts SET encrypted_cookie=?1,status='idle' WHERE id=?2", params![blob, id])?;
            return Ok(id);
        }
        c.execute(
            "INSERT INTO accounts(label,username,encrypted_cookie,created_at) VALUES(?1,?1,?2,?3)",
            params![username, blob, now()],
        )?;
        Ok(c.last_insert_rowid())
    }

    pub fn list_accounts(&self) -> Res<Vec<Account>> {
        let c = self.c();
        let mut st = c.prepare(&format!("SELECT {COLS} FROM accounts ORDER BY label COLLATE NOCASE"))?;
        let v = st.query_map([], row_acc)?.collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    pub fn get_account(&self, id: i64) -> Res<Account> {
        Ok(self.c().query_row(&format!("SELECT {COLS} FROM accounts WHERE id=?1"), [id], row_acc)?)
    }

    pub fn get_cookie(&self, id: i64) -> Res<Zeroizing<String>> {
        let blob: Vec<u8> =
            self.c().query_row("SELECT encrypted_cookie FROM accounts WHERE id=?1", [id], |r| r.get(0))?;
        if blob.is_empty() {
            return Err("no cookie stored".into());
        }
        let plain = crypto::unprotect(&blob)?;
        Ok(Zeroizing::new(String::from_utf8(plain.to_vec())?))
    }

    /// Sets the game; `name` is its display name when known (picked from search), else "".
    pub fn set_place(&self, id: i64, v: &str) -> Res<()> {
        self.set_place_named(id, v, "")
    }
    pub fn set_place_named(&self, id: i64, v: &str, name: &str) -> Res<()> {
        self.c().execute("UPDATE accounts SET place_id=?1, place_name=?2 WHERE id=?3", params![v, name, id])?;
        Ok(())
    }
    pub fn set_label(&self, id: i64, v: &str) -> Res<()> {
        self.c().execute("UPDATE accounts SET label=?1 WHERE id=?2", params![v, id])?;
        Ok(())
    }
    pub fn set_auto_start(&self, id: i64, on: bool) {
        let _ = self.c().execute("UPDATE accounts SET auto_start=?1 WHERE id=?2", params![on as i64, id]);
    }
    pub fn set_group(&self, id: i64, v: &str) -> Res<()> {
        self.c().execute("UPDATE accounts SET group_tag=?1 WHERE id=?2", params![v, id])?;
        Ok(())
    }
    pub fn set_preset(&self, id: i64, v: i64) -> Res<()> {
        self.c().execute("UPDATE accounts SET preset_id=?1 WHERE id=?2", params![v, id])?;
        Ok(())
    }
    pub fn set_status(&self, id: i64, v: &str) -> Res<()> {
        self.c().execute("UPDATE accounts SET status=?1 WHERE id=?2", params![v, id])?;
        Ok(())
    }
    pub fn set_job(&self, id: i64, v: Option<&str>) -> Res<()> {
        self.c().execute("UPDATE accounts SET last_job_id=?1 WHERE id=?2", params![v, id])?;
        Ok(())
    }
    pub fn touch_used(&self, id: i64) {
        let _ = self.c().execute("UPDATE accounts SET last_used=?1 WHERE id=?2", params![now(), id]);
    }
    pub fn reset_statuses(&self) {
        let _ = self.c().execute("UPDATE accounts SET status='idle' WHERE status<>'needs_relogin'", []);
    }
    // ---------- Roblox user ids ----------
    pub fn set_user_id(&self, id: i64, uid: u64) {
        let _ = self.c().execute("UPDATE accounts SET user_id=?1 WHERE id=?2", params![uid as i64, id]);
    }
    /// (account id, username) of accounts whose user id isn't known yet.
    pub fn missing_user_ids(&self) -> Vec<(i64, String)> {
        let c = self.c();
        let Ok(mut st) = c.prepare("SELECT id,username FROM accounts WHERE user_id IS NULL OR user_id=0") else { return vec![] };
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map(|it| it.flatten().collect()).unwrap_or_default()
    }
    pub fn user_id_of(&self, id: i64) -> Option<u64> {
        self.c()
            .query_row("SELECT user_id FROM accounts WHERE id=?1", [id], |r| r.get::<_, Option<i64>>(0))
            .ok()
            .flatten()
            .filter(|v| *v > 0)
            .map(|v| v as u64)
    }
    pub fn account_by_user_id(&self, uid: u64) -> Option<i64> {
        self.c().query_row("SELECT id FROM accounts WHERE user_id=?1", [uid as i64], |r| r.get(0)).ok()
    }

    // ---------- running clients (survive a manager restart) ----------
    pub fn save_live(&self, id: i64, pid: u32, start_time: u64, launch_ts: u64, live_since: i64) {
        let _ = self.c().execute(
            "INSERT OR REPLACE INTO live_clients(account_id,pid,start_time,launch_ts,live_since) VALUES(?1,?2,?3,?4,?5)",
            params![id, pid, start_time as i64, launch_ts as i64, live_since],
        );
    }
    pub fn del_live(&self, id: i64) {
        let _ = self.c().execute("DELETE FROM live_clients WHERE account_id=?1", [id]);
    }
    /// (account id, pid, start time, launch ts, live since)
    pub fn list_live(&self) -> Vec<(i64, u32, u64, u64, i64)> {
        let c = self.c();
        let Ok(mut st) = c.prepare("SELECT account_id,pid,start_time,launch_ts,live_since FROM live_clients") else {
            return vec![];
        };
        st.query_map([], |r| {
            Ok((r.get(0)?, r.get::<_, i64>(1)? as u32, r.get::<_, i64>(2)? as u64, r.get::<_, i64>(3)? as u64, r.get(4)?))
        })
        .map(|it| it.flatten().collect())
        .unwrap_or_default()
    }

    pub fn delete_account(&self, id: i64) -> Res<()> {
        let c = self.c();
        c.execute("DELETE FROM live_clients WHERE account_id=?1", [id])?;
        c.execute("DELETE FROM accounts WHERE id=?1", [id])?;
        c.execute("DELETE FROM launch_events WHERE account_id=?1", [id])?;
        Ok(())
    }
    pub fn clear_all_cookies(&self) -> Res<()> {
        self.c().execute("UPDATE accounts SET encrypted_cookie=x'', status='needs_relogin'", [])?;
        Ok(())
    }

    // ---------- presets ----------
    pub fn list_presets(&self) -> Res<Vec<Preset>> {
        let c = self.c();
        let mut st = c.prepare("SELECT id,name,flags_json,priority,trim_interval_sec FROM presets ORDER BY id")?;
        let v = st
            .query_map([], |r| {
                let j: String = r.get(2)?;
                Ok(Preset {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    gfx: serde_json::from_str(&j).unwrap_or_default(),
                    priority: r.get(3)?,
                    trim_interval_sec: r.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }
    pub fn get_preset(&self, id: i64) -> Res<Preset> {
        self.list_presets()?.into_iter().find(|p| p.id == id).ok_or_else(|| "preset not found".into())
    }
    pub fn save_preset(&self, p: &Preset) -> Res<()> {
        self.c().execute(
            "UPDATE presets SET name=?1,flags_json=?2,priority=?3 WHERE id=?4",
            params![p.name, serde_json::to_string(&p.gfx)?, p.priority, p.id],
        )?;
        Ok(())
    }
    pub fn new_preset(&self, name: &str, g: &Gfx, priority: i32) -> Res<i64> {
        let c = self.c();
        c.execute(
            "INSERT INTO presets(name,flags_json,priority,trim_interval_sec) VALUES(?1,?2,?3,0)",
            params![name, serde_json::to_string(g)?, priority],
        )?;
        Ok(c.last_insert_rowid())
    }

    // ---------- settings ----------
    pub fn load_settings(&self) -> Settings {
        let mut s: Settings = self
            .c()
            .query_row("SELECT value FROM settings WHERE key='app'", [], |r| r.get::<_, String>(0))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if std::mem::take(&mut s.trim_enabled) && s.trim_mode == TrimMode::Off {
            s.trim_mode = TrimMode::Balanced;
        }
        s.trim_target_mb = s.trim_target_mb.clamp(60, 2000);
        s
    }
    pub fn save_settings(&self, s: &Settings) -> Res<()> {
        self.c().execute(
            "INSERT INTO settings(key,value) VALUES('app',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [serde_json::to_string(s)?],
        )?;
        Ok(())
    }

    pub fn get_kv(&self, key: &str) -> Option<String> {
        self.c().query_row("SELECT value FROM settings WHERE key=?1", [key], |r| r.get(0)).ok()
    }
    pub fn set_kv(&self, key: &str, value: &str) -> Res<()> {
        self.c().execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
    pub fn del_kv(&self, key: &str) -> Res<()> {
        self.c().execute("DELETE FROM settings WHERE key=?1", [key])?;
        Ok(())
    }

    // ---------- events ----------
    /// Unix seconds of every launch this manager performed (used to never "verify" against our own URIs).
    pub fn own_launch_times(&self) -> Vec<i64> {
        let c = self.c();
        let Ok(mut st) = c.prepare("SELECT ts FROM launch_events WHERE event='launch'") else { return vec![] };
        let v: Vec<i64> = st.query_map([], |r| r.get::<_, i64>(0)).map(|it| it.flatten().collect()).unwrap_or_default();
        v
    }
    pub fn log_event(&self, account_id: i64, event: &str, detail: &str) -> Res<()> {
        self.c().execute(
            "INSERT INTO launch_events(account_id,ts,event,detail) VALUES(?1,?2,?3,?4)",
            params![account_id, now(), event, detail],
        )?;
        Ok(())
    }
    pub fn recent_events(&self, limit: i64) -> Res<Vec<Ev>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT e.account_id,e.ts,COALESCE(a.label,'—'),e.event,e.detail FROM launch_events e
             LEFT JOIN accounts a ON a.id=e.account_id ORDER BY e.id DESC LIMIT ?1",
        )?;
        let mut v = st
            .query_map([limit], |r| {
                Ok(Ev { account_id: r.get(0)?, ts: r.get(1)?, label: r.get(2)?, event: r.get(3)?, detail: r.get(4)? })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        v.reverse();
        Ok(v)
    }

    // ---------- backup (cookies stay DPAPI-encrypted; only restorable by same Windows user) ----------
    pub fn export_backup(&self) -> Res<PathBuf> {
        let c = self.c();
        let mut st = c.prepare("SELECT label,username,place_id,encrypted_cookie FROM accounts")?;
        let rows = st
            .query_map([], |r| {
                let blob: Vec<u8> = r.get(3)?;
                let hex: String = blob.iter().map(|b| format!("{b:02x}")).collect();
                Ok(serde_json::json!({"label":r.get::<_,String>(0)?,"username":r.get::<_,String>(1)?,
                    "place_id":r.get::<_,String>(2)?,"encrypted_cookie_hex":hex}))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let path = Self::dir().join("backup.json");
        std::fs::write(&path, serde_json::to_string_pretty(&rows)?)?;
        Ok(path)
    }
}
