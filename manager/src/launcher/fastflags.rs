use crate::storage::db::Gfx;
use serde_json::{Map, Value};
use std::{fs, path::PathBuf, time::UNIX_EPOCH};

pub fn current_version() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var("LOCALAPPDATA").ok()?).join("Roblox").join("Versions");
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for e in fs::read_dir(root).ok()?.flatten() {
        let exe = e.path().join("RobloxPlayerBeta.exe");
        if exe.is_file() {
            let m = exe.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH);
            if best.as_ref().map_or(true, |(t, _)| m > *t) {
                best = Some((m, e.path()));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// None => remove the key (setting is "off"), so we never leave stale values behind.
fn to_flags(g: &Gfx) -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("DFIntTaskSchedulerTargetFps", Some(Value::from(g.fps))),
        ("FIntDebugTextureManagerSkipMips", Some(Value::from(g.skip_mips))),
        ("FFlagDisablePostFx", g.post_fx_off.then(|| Value::Bool(true))),
        ("FIntRenderShadowIntensity", g.shadows_off.then(|| Value::from(0))),
        ("DFIntMaxFrameBufferSize", (g.fb_cap > 0).then(|| Value::from(g.fb_cap))),
    ]
}

/// MERGES into any existing ClientAppSettings.json; unrelated flags are preserved.
pub fn apply(g: &Gfx) -> Result<(), String> {
    let dir = current_version().ok_or_else(|| "Roblox player not found in %LOCALAPPDATA%\\Roblox\\Versions".to_string())?.join("ClientSettings");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join("ClientAppSettings.json");
    let mut map: Map<String, Value> =
        fs::read_to_string(&file).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    for (k, v) in to_flags(g) {
        match v {
            Some(v) => {
                map.insert(k.to_string(), v);
            }
            None => {
                map.remove(k);
            }
        }
    }
    fs::write(&file, serde_json::to_string_pretty(&map).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}
