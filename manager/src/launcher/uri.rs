pub fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// Plain launch (Roblox picks the server). This is the only URI built from assumptions; it is the
/// format the manager has always used. Same-server joins never use this builder — they are built by
/// `uri_probe::build_rejoin` from a captured, verified template.
/// `place` must be digits only (validated by caller).
/// No game: opens the signed-in Roblox app on its home screen ("launchmode:app", the same mode
/// roblox.com uses to open the app without joining anything).
pub fn build_app(ticket: &str, ts_secs: u64) -> String {
    format!("roblox-player:1+launchmode:app+gameinfo:{ticket}+launchtime:{}", ts_secs * 1000)
}

pub fn build(ticket: &str, ts_secs: u64, place: &str) -> String {
    let place: String = place.chars().filter(|c| c.is_ascii_digit()).collect();
    let url = format!("https://assetgame.roblox.com/game/placelauncher.ashx?request=RequestGame&placeId={place}");
    format!(
        "roblox-player:1+launchmode:play+gameinfo:{ticket}+launchtime:{}+placelauncherurl:{}",
        ts_secs * 1000,
        pct(&url)
    )
}
