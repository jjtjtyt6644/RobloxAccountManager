//! Evidence gate for same-server rejoin.
//!
//! We do NOT guess the RequestGameJob URI format or the `launchtime` unit. Instead we look in Roblox's
//! own log folder for a `roblox-player:1+...` URI that Roblox launched itself (e.g. the user clicked
//! "Join" on a specific server on roblox.com), and require ALL of:
//!   * outer keys include launchmode, gameinfo, launchtime, placelauncherurl
//!   * launchtime is 13 digits (ms) or 10 digits (s) — the unit is taken from the capture
//!   * placelauncherurl decodes to a URL with request=RequestGameJob and a gameId parameter
//!   * its launchtime is NOT within 15 s of any launch this manager made (so we never "verify"
//!     against our own guessed output)
//! Only then is a `UriSchema` stored and the rejoin option unlocked. The schema records the exact
//! key order and every non-dynamic value verbatim; the ticket (gameinfo) is never stored.
use super::uri::pct;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use zeroize::Zeroizing;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct UriSchema {
    /// Outer `key:value` pairs in captured order. Values for gameinfo/launchtime/placelauncherurl are blank.
    pub keys: Vec<(String, String)>,
    pub launchtime_ms: bool,
    /// Captured placelauncherurl without its query, e.g. https://assetgame.roblox.com/game/PlaceLauncher.ashx
    pub job_url_base: String,
    /// Captured query params in order. placeId/gameId/joinAttemptId values are blank (filled per launch).
    pub job_params: Vec<(String, String)>,
    pub source_file: String,
    pub captured_at: i64,
}

#[derive(Default)]
pub struct ProbeReport {
    pub schema: Option<UriSchema>,
    pub files_scanned: usize,
    pub uris_seen: usize,
    pub own_skipped: usize,
    pub plain_only: usize,
}

impl ProbeReport {
    pub fn summary(&self) -> String {
        match &self.schema {
            Some(s) => format!(
                "Verified from {} — launchtime in {}, {} launch parameters, {} server-join parameters.",
                s.source_file,
                if s.launchtime_ms { "milliseconds" } else { "seconds" },
                s.keys.len(),
                s.job_params.len()
            ),
            None if self.uris_seen == 0 => format!(
                "Not verified: no Roblox launch link found in {} log file(s). Join any server from the \
                 roblox.com Servers list once, then check again.",
                self.files_scanned
            ),
            None => format!(
                "Not verified: found {} launch link(s) ({} were this manager's own, {} were normal joins), but \
                 none was a join to a specific server. On roblox.com open a game > Servers > Join, then check again.",
                self.uris_seen, self.own_skipped, self.plain_only
            ),
        }
    }
}

fn pct_decode(s: &str) -> String {
    fn hex(c: u8) -> Option<u8> {
        (c as char).to_digit(16).map(|d| d as u8)
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse(uri: &str, own: &[i64]) -> Result<Option<UriSchema>, &'static str> {
    let mut parts = uri.split('+');
    if parts.next() != Some("roblox-player:1") {
        return Err("bad prefix");
    }
    let mut keys = Vec::new();
    let mut launchtime_ms = None;
    let mut plu = None;
    for p in parts {
        let Some((k, v)) = p.split_once(':') else { continue };
        match k {
            "gameinfo" => keys.push((k.to_string(), String::new())),
            "launchtime" => {
                if !v.bytes().all(|c| c.is_ascii_digit()) {
                    return Err("launchtime not numeric");
                }
                let (ms, secs) = match v.len() {
                    13 => (true, v.parse::<i64>().map_err(|_| "launchtime")? / 1000),
                    10 => (false, v.parse::<i64>().map_err(|_| "launchtime")?),
                    _ => return Err("launchtime unit unrecognised"),
                };
                if own.iter().any(|o| (o - secs).abs() <= 15) {
                    return Err("own");
                }
                launchtime_ms = Some(ms);
                keys.push((k.to_string(), String::new()));
            }
            "placelauncherurl" => {
                plu = Some(pct_decode(v));
                keys.push((k.to_string(), String::new()));
            }
            _ => keys.push((k.to_string(), v.to_string())),
        }
    }
    let have = |n: &str| keys.iter().any(|(k, _)| k == n);
    if !(have("launchmode") && have("gameinfo") && have("launchtime") && have("placelauncherurl")) {
        return Err("missing keys");
    }
    let (Some(ms), Some(url)) = (launchtime_ms, plu) else { return Err("missing keys") };
    let Some((base, query)) = url.split_once('?') else { return Err("no query") };
    let params: Vec<(String, String)> = query
        .split('&')
        .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();
    let get = |n: &str| params.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.as_str());
    if get("request") != Some("RequestGameJob") || get("gameId").is_none() || get("placeId").is_none() {
        return Ok(None); // a real Roblox launch, but not a specific-server join
    }
    let job_params = params
        .into_iter()
        .map(|(k, v)| {
            let blank = ["placeid", "gameid", "joinattemptid"].contains(&k.to_ascii_lowercase().as_str());
            (k, if blank { String::new() } else { v })
        })
        .collect();
    Ok(Some(UriSchema {
        keys,
        launchtime_ms: ms,
        job_url_base: base.to_string(),
        job_params,
        source_file: String::new(),
        captured_at: 0,
    }))
}

/// Blocking file IO (reads up to 60 newest logs, 1 MB each). Run off the UI thread.
pub fn probe(own_launch_secs: &[i64]) -> ProbeReport {
    let mut rep = ProbeReport::default();
    let Some(dir) = crate::watcher::crash_detect::logs_dir() else { return rep };
    let Ok(rd) = fs::read_dir(&dir) else { return rep };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter(|e| e.path().extension().map_or(false, |x| x == "log"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    const MARK: &str = "roblox-player:1+";
    for (_, path) in files.into_iter().take(60) {
        rep.files_scanned += 1;
        let Ok(f) = fs::File::open(&path) else { continue };
        let mut buf = Zeroizing::new(Vec::new()); // logs may contain tickets — wipe after use
        if f.take(1 << 20).read_to_end(&mut buf).is_err() {
            continue;
        }
        let text = Zeroizing::new(String::from_utf8_lossy(&buf).into_owned());
        let mut from = 0;
        while let Some(i) = text[from..].find(MARK) {
            let start = from + i;
            let end = text[start..]
                .find(|c: char| c.is_whitespace() || "\"'<>,;)]}".contains(c))
                .map_or(text.len(), |n| start + n);
            from = end;
            rep.uris_seen += 1;
            match parse(&text[start..end], own_launch_secs) {
                Err("own") => rep.own_skipped += 1,
                Err(_) => {}
                Ok(None) => rep.plain_only += 1,
                Ok(Some(mut s)) => {
                    s.source_file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    s.captured_at = crate::storage::db::now();
                    rep.schema = Some(s);
                    return rep;
                }
            }
        }
    }
    rep
}

fn new_guid() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let a = RandomState::new().hash_one(t);
    let b = RandomState::new().hash_one(t ^ 0x9e37_79b9_7f4a_7c15);
    let mut x = ((a as u128) << 64) | b as u128;
    x = (x & !(0xF000u128 << 64)) | (0x4000u128 << 64); // version 4
    x = (x & !(0xC000u128 << 48)) | (0x8000u128 << 48); // RFC 4122 variant
    let h = format!("{x:032x}");
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// Build a same-server join URI by filling the captured template. `job` must be a GUID and `place`
/// digits only (both validated by callers).
pub fn build_rejoin(s: &UriSchema, ticket: &str, place: &str, job: &str) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let lt = if s.launchtime_ms { now.as_millis() } else { now.as_secs() as u128 };
    let q: Vec<String> = s
        .job_params
        .iter()
        .map(|(k, v)| {
            let val = match k.to_ascii_lowercase().as_str() {
                "placeid" => place.to_string(),
                "gameid" => job.to_string(),
                "joinattemptid" => new_guid(),
                _ => v.clone(),
            };
            format!("{k}={val}")
        })
        .collect();
    let url = format!("{}?{}", s.job_url_base, q.join("&"));
    let mut out = String::from("roblox-player:1");
    for (k, v) in &s.keys {
        let val = match k.as_str() {
            "gameinfo" => ticket.to_string(),
            "launchtime" => lt.to_string(),
            "placelauncherurl" => pct(&url),
            _ => v.clone(),
        };
        out.push('+');
        out.push_str(k);
        out.push(':');
        out.push_str(&val);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_plain_and_own_accepts_job() {
        let job = "roblox-player:1+launchmode:play+gameinfo:TICKET+launchtime:1700000000000+placelauncherurl:https%3A%2F%2Fassetgame.roblox.com%2Fgame%2FPlaceLauncher.ashx%3Frequest%3DRequestGameJob%26browserTrackerId%3D42%26placeId%3D1%26gameId%3Dabc%26isPlayTogetherGame%3Dfalse+browsertrackerid:42+robloxLocale:en_us";
        let s = parse(job, &[]).unwrap().unwrap();
        assert!(s.launchtime_ms);
        assert_eq!(s.keys[0].0, "launchmode");
        assert!(s.keys.iter().all(|(k, v)| k != "gameinfo" || v.is_empty()));
        assert!(matches!(parse(job, &[1_700_000_005]), Err("own")));
        let plain = job.replace("RequestGameJob", "RequestGame");
        assert!(parse(&plain, &[]).unwrap().is_none());
        let out = build_rejoin(&s, "T2", "99", "11111111-2222-3333-4444-555555555555");
        assert!(out.contains("gameinfo:T2") && out.contains("gameId%3D11111111") && out.contains("placeId%3D99"));
        assert!(out.contains("browsertrackerid:42") && !out.contains("TICKET"));
    }
}
