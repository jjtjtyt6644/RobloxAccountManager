//! Find a game's Place ID by name (the account page's Game box).
//!
//! One request to Roblox's own search (the one roblox.com's search bar uses), made only after you stop
//! typing, on a background task. Results are a few names and numbers; no thumbnails are downloaded,
//! so there's no lasting memory, CPU or GPU cost.
use crate::launcher::Engine;

#[derive(Clone)]
pub struct GameHit {
    pub name: String,
    pub place_id: String,
    pub players: u64,
}

#[derive(Clone, Default)]
pub struct SearchState {
    /// The text these results are for (results for an older query are never shown for a newer one).
    pub query: String,
    pub busy: bool,
    pub results: Vec<GameHit>,
    pub error: Option<String>,
}

const MAX_RESULTS: usize = 8;

async fn search(http: &reqwest::Client, q: &str) -> Result<Vec<GameHit>, String> {
    let session = format!("{:016x}{:016x}", std::process::id(), crate::storage::db::now());
    let r = http
        .get("https://apis.roblox.com/search-api/omni-search")
        .query(&[("searchQuery", q), ("sessionId", session.as_str()), ("pageType", "all")])
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("couldn't reach Roblox search: {e}"))?;
    let body = r.bytes().await.map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|_| "unexpected reply from Roblox search".to_string())?;
    let mut out = vec![];
    for group in v["searchResults"].as_array().into_iter().flatten() {
        if group["contentGroupType"].as_str() != Some("Game") {
            continue;
        }
        for c in group["contents"].as_array().into_iter().flatten() {
            let Some(place) = c["rootPlaceId"].as_u64() else { continue };
            let name: String = c["name"].as_str().unwrap_or("?").chars().filter(|ch| !ch.is_control()).collect();
            out.push(GameHit { name: name.trim().to_string(), place_id: place.to_string(), players: c["playerCount"].as_u64().unwrap_or(0) });
            if out.len() >= MAX_RESULTS {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

impl Engine {
    pub fn search_games(&self, q: String) {
        {
            let mut s = self.search.lock().unwrap();
            *s = SearchState { query: q.clone(), busy: true, ..Default::default() };
        }
        self.ui.repaint();
        let e = self.clone();
        self.rt.spawn(async move {
            let res = search(&e.http, &q).await;
            let mut s = e.search.lock().unwrap();
            if s.query != q {
                return; // you kept typing; a newer search owns the state
            }
            s.busy = false;
            match res {
                Ok(r) if r.is_empty() => s.error = Some(format!("No games found for \u{201c}{q}\u{201d}.")),
                Ok(r) => s.results = r,
                Err(er) => s.error = Some(er),
            }
            drop(s);
            e.ui.repaint();
        });
    }
}

#[cfg(test)]
mod tests {
    /// Hits Roblox's real search, so not run by default: `cargo test -p roblox_account_manager search_finds_adopt_me -- --ignored`
    #[test]
    #[ignore]
    fn search_finds_adopt_me() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let http = reqwest::Client::builder().user_agent("Mozilla/5.0").build().unwrap();
        let hits = rt.block_on(super::search(&http, "adopt me")).unwrap();
        for h in &hits {
            println!("{:<40} place {:<16} {} playing", h.name, h.place_id, h.players);
        }
        assert!(hits.iter().any(|h| h.place_id == "920587237"), "Adopt Me! (920587237) should be found");
    }
}
