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

/// Only flags on Roblox's allow-list for local configuration. Everything else is "Denied local
/// configuration" (it says so in its log) — including the frame-rate, texture-mip, post-effects and
/// shadow flags older versions wrote, which are removed here. Frame-rate cap and graphics quality go
/// through Roblox's own settings instead (launcher::basic_settings).
/// None => remove the key, so we never leave stale values behind.
fn to_flags(g: &Gfx) -> Vec<(&'static str, Option<Value>)> {
    let tex = match g.skip_mips {
        0 => None,
        1 => Some(2),
        2 => Some(1),
        _ => Some(0),
    };
    vec![
        ("DFFlagTextureQualityOverrideEnabled", tex.map(|_| Value::Bool(true))),
        ("DFIntTextureQualityOverride", tex.map(Value::from)),
        ("FIntDebugForceMSAASamples", g.msaa_off.then(|| Value::from(0))),
        ("FIntFRMMaxGrassDistance", g.grass_off.then(|| Value::from(0))),
        ("FIntFRMMinGrassDistance", g.grass_off.then(|| Value::from(0))),
        ("DFIntDebugFRMQualityLevelOverride", (1..=10).contains(&g.graphics_quality).then(|| Value::from(g.graphics_quality))),
        // Denied by Roblox; written by versions before 1.7.0. Always removed.
        ("DFIntTaskSchedulerTargetFps", None),
        ("FIntDebugTextureManagerSkipMips", None),
        ("FFlagDisablePostFx", None),
        ("FIntRenderShadowIntensity", None),
        ("DFIntMaxFrameBufferSize", None),
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
