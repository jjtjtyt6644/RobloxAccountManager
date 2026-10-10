//! Roblox's own in-game settings (%LOCALAPPDATA%\Roblox\GlobalBasicSettings_13.xml): frame-rate cap,
//! graphics quality and volume.
//!
//! Since late 2024 Roblox ignores most ClientAppSettings.json flags ("Denied local configuration for:
//! DFIntTaskSchedulerTargetFps" in its log), so a profile's frame-rate cap and quality are applied
//! through the same settings the in-game menu changes. The file is shared by every client on the PC and
//! read when a client starts, so: write the profile's values just before a launch (inside the launch
//! gate, one client at a time), and put the user's own values back once that client is up
//! (`Restore` does it on drop, so every early return restores too).
use crate::storage::db::Gfx;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

/// The user's own values for the settings we touch, as last seen before a launch changed them.
static USER: Mutex<Option<HashMap<&'static str, String>>> = Mutex::new(None);

fn path() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var("LOCALAPPDATA").ok()?).join("Roblox");
    fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            n.starts_with("GlobalBasicSettings_") && n.ends_with(".xml") && !n.contains("Studio")
        })
        .max_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok())
}

/// `<int name="FramerateCap">60</int>` -> ("int", "60"); None if the setting isn't in the file.
fn get(xml: &str, name: &str) -> Option<(usize, usize)> {
    let key = format!(" name=\"{name}\">");
    let i = xml.find(&key)? + key.len();
    let j = i + xml[i..].find('<')?;
    Some((i, j))
}

fn set(xml: &mut String, name: &str, value: &str) -> Option<String> {
    let (i, j) = get(xml, name)?;
    let old = xml[i..j].to_string();
    xml.replace_range(i..j, value);
    Some(old)
}

/// The values the profile wants (only ones it actually sets).
fn wanted(g: &Gfx) -> Vec<(&'static str, String)> {
    let mut v = vec![];
    match g.fps {
        0 => {}
        f if f >= 240 => v.push(("FramerateCap", "-1".to_string())), // uncapped
        f => v.push(("FramerateCap", f.to_string())),
    }
    if (1..=10).contains(&g.graphics_quality) {
        // The menu's 10 steps map onto the engine's 21 levels.
        let q = g.graphics_quality;
        v.push(("SavedQualityLevel", q.to_string()));
        v.push(("GraphicsQualityLevel", ((q - 1) * 20 / 9 + 1).to_string()));
    }
    if g.mute {
        v.push(("MasterVolume", "0".to_string()));
    }
    v
}

/// Puts the user's own values back when dropped.
pub struct Restore {
    file: Option<PathBuf>,
    old: Vec<(&'static str, String)>,
}

impl Drop for Restore {
    fn drop(&mut self) {
        let Some(file) = &self.file else { return };
        if self.old.is_empty() {
            return;
        }
        let Ok(mut xml) = fs::read_to_string(file) else { return };
        for (k, v) in &self.old {
            set(&mut xml, k, v);
        }
        let _ = fs::write(file, xml);
    }
}

/// Write the profile's values. Never fails a launch: if the file is missing or unreadable the client
/// just starts with the user's own settings.
pub fn apply(g: &Gfx) -> Restore {
    let none = Restore { file: None, old: vec![] };
    let want = wanted(g);
    if want.is_empty() {
        return none;
    }
    let Some(file) = path() else { return none };
    let Ok(mut xml) = fs::read_to_string(&file) else { return none };
    let mut old = vec![];
    for (k, v) in want {
        if let Some(prev) = set(&mut xml, k, &v) {
            // Only a value that isn't this profile's own is the user's (a closing client may have
            // left a profile's value behind; that must never become "the user's setting").
            if prev != v {
                USER.lock().unwrap().get_or_insert_with(HashMap::new).insert(k, prev.clone());
            }
            old.push((k, prev));
        }
    }
    if fs::write(&file, &xml).is_err() {
        return none;
    }
    Restore { file: Some(file), old }
}

/// After a client using `g` has closed: if it saved its profile's values back into the shared file
/// (clients write their settings when they close), put the user's own values back.
pub fn undo_after_exit(g: &Gfx) {
    let want = wanted(g);
    if want.is_empty() {
        return;
    }
    let Some(user) = USER.lock().unwrap().clone() else { return };
    let Some(file) = path() else { return };
    let Ok(mut xml) = fs::read_to_string(&file) else { return };
    let mut changed = false;
    for (k, v) in want {
        let cur = get(&xml, k).map(|(i, j)| xml[i..j].to_string());
        if let (Some(cur), Some(mine)) = (cur, user.get(k)) {
            if cur == v && *mine != v {
                set(&mut xml, k, mine);
                changed = true;
            }
        }
    }
    if changed {
        let _ = fs::write(&file, xml);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edits_only_existing_values() {
        let mut xml = String::from(
            "<Item><Properties><int name=\"FramerateCap\">-1</int><token name=\"SavedQualityLevel\">10</token>\
             <int name=\"GraphicsQualityLevel\">21</int><float name=\"MasterVolume\">0.5</float></Properties></Item>",
        );
        let g = Gfx { fps: 30, graphics_quality: 1, mute: true, ..Gfx::default() };
        let mut old = vec![];
        for (k, v) in wanted(&g) {
            old.push((k, set(&mut xml, k, &v).unwrap()));
        }
        assert!(xml.contains("\"FramerateCap\">30<") && xml.contains("\"SavedQualityLevel\">1<"));
        assert!(xml.contains("\"GraphicsQualityLevel\">1<") && xml.contains("\"MasterVolume\">0<"));
        for (k, v) in &old {
            set(&mut xml, k, v);
        }
        assert!(xml.contains("\"FramerateCap\">-1<") && xml.contains("\"GraphicsQualityLevel\">21<") && xml.contains("\"MasterVolume\">0.5<"));
        assert!(set(&mut xml, "NotThere", "1").is_none());
    }
}
