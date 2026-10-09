use reqwest::{header::HeaderValue, Client};

#[derive(Debug)]
pub enum TicketErr {
    Unauthorized,
    RateLimited,
    Other(String),
}

fn ck(cookie: &str) -> HeaderValue {
    let mut v = HeaderValue::from_str(&format!(".ROBLOSECURITY={cookie}"))
        .unwrap_or_else(|_| HeaderValue::from_static(""));
    v.set_sensitive(true); // keeps it out of Debug output
    v
}

/// Two-step CSRF dance: POST -> 403 + X-CSRF-TOKEN -> POST again -> ticket header.
pub async fn fetch(http: &Client, cookie: &str, place: &str) -> Result<String, TicketErr> {
    let url = "https://auth.roblox.com/v1/authentication-ticket";
    let referer = format!("https://www.roblox.com/games/{place}");
    let mut csrf = String::new();
    for _ in 0..2 {
        let mut rq = http
            .post(url)
            .header("Cookie", ck(cookie))
            .header("Referer", &referer)
            .header("Content-Type", "application/json")
            .body("{}");
        if !csrf.is_empty() {
            rq = rq.header("X-CSRF-TOKEN", &csrf);
        }
        let r = rq.send().await.map_err(|e| TicketErr::Other(e.to_string()))?;
        match r.status().as_u16() {
            200 => {
                return r
                    .headers()
                    .get("rbx-authentication-ticket")
                    .and_then(|v| v.to_str().ok())
                    .map(String::from)
                    .ok_or_else(|| TicketErr::Other("ticket header missing".into()))
            }
            401 => return Err(TicketErr::Unauthorized),
            429 => return Err(TicketErr::RateLimited),
            403 => {
                csrf = r.headers().get("x-csrf-token").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                if csrf.is_empty() {
                    return Err(TicketErr::Other("no CSRF token in 403".into()));
                }
            }
            s => return Err(TicketErr::Other(format!("unexpected status {s}"))),
        }
    }
    Err(TicketErr::Other("CSRF exchange failed".into()))
}

/// Validates a cookie and returns the account's (user id, username).
pub async fn whoami(http: &Client, cookie: &str) -> Result<(u64, String), String> {
    let r = http
        .get("https://users.roblox.com/v1/users/authenticated")
        .header("Cookie", ck(cookie))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("cookie rejected (status {})", r.status().as_u16()));
    }
    let v: serde_json::Value = serde_json::from_str(&r.text().await.map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let name = v["name"].as_str().map(String::from).ok_or("no username in response")?;
    Ok((v["id"].as_u64().unwrap_or(0), name))
}

/// Public lookup (no sign-in needed): username -> user id, for accounts added before user ids were stored.
pub async fn user_ids(http: &Client, usernames: &[String]) -> Result<Vec<(String, u64)>, String> {
    let body = serde_json::json!({ "usernames": usernames, "excludeBannedUsers": false }).to_string();
    let r = http
        .post("https://users.roblox.com/v1/usernames/users")
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("lookup failed (status {})", r.status().as_u16()));
    }
    let v: serde_json::Value = serde_json::from_str(&r.text().await.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(v["data"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| Some((x["requestedUsername"].as_str()?.to_string(), x["id"].as_u64()?)))
                .collect()
        })
        .unwrap_or_default())
}
