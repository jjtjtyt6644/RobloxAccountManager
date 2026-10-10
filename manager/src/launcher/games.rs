//! Names of the games accounts are playing ("Playing Adopt Me!").
//!
//! A client's log records the game's universe id when it joins ("…universeid:383310974…"); one
//! request to Roblox's public games API turns that into a name. Names are cached for the session, so
//! each game is looked up once, and only when a window joins it.
use crate::launcher::Engine;

/// "[6H⏳] Adopt Me!" -> "Adopt Me!": drops the update/event tags games put in front of their names.
pub fn clean_name(name: &str) -> String {
    let mut s = name.trim();
    loop {
        let t = s.trim_start();
        let close = match t.chars().next() {
            Some('[') => ']',
            Some('(') => ')',
            Some('{') => '}',
            _ => break,
        };
        match t.find(close) {
            Some(i) if i + 1 < t.len() => s = &t[i + close.len_utf8()..],
            _ => break,
        }
    }
    let s = s.trim();
    if s.is_empty() { name.trim().to_string() } else { s.to_string() }
}

impl Engine {
    /// The name of `universe`, or None while it's being looked up (the lookup starts on first ask).
    pub fn game_name(&self, universe: u64) -> Option<String> {
        {
            let mut names = self.game_names.lock().unwrap();
            match names.get(&universe) {
                Some(n) => return n.clone(),
                None => {
                    names.insert(universe, None); // in flight
                }
            }
        }
        let e = self.clone();
        self.rt.spawn(async move {
            let url = format!("https://games.roblox.com/v1/games?universeIds={universe}");
            let name = async {
                let r = e.http.get(&url).timeout(std::time::Duration::from_secs(10)).send().await.ok()?;
                let v: serde_json::Value = serde_json::from_slice(&r.bytes().await.ok()?).ok()?;
                v["data"][0]["name"].as_str().map(clean_name)
            }
            .await;
            match name {
                Some(n) => {
                    e.game_names.lock().unwrap().insert(universe, Some(n));
                    e.ui.repaint();
                }
                None => {
                    e.game_names.lock().unwrap().remove(&universe); // try again next time it's asked
                }
            }
        });
        None
    }

    /// What account `id` is playing right now, if known.
    pub fn now_playing(&self, id: i64) -> Option<String> {
        let u = self.tr().get(&id).filter(|t| t.pid.is_some()).and_then(|t| t.universe)?;
        self.game_name(u)
    }
}

#[cfg(test)]
mod tests {
    use super::clean_name;
    #[test]
    fn cleans_tags() {
        assert_eq!(clean_name("[6H⏳] Adopt Me!"), "Adopt Me!");
        assert_eq!(clean_name("[🎃] Parenthood 👶 Beta"), "Parenthood 👶 Beta");
        assert_eq!(clean_name("Brookhaven 🏡RP"), "Brookhaven 🏡RP");
        assert_eq!(clean_name("[UPDATE]"), "[UPDATE]");
    }
}
