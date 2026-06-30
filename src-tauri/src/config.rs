use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelayConfig {
    pub supabase_url: String,
    pub supabase_key: String,
    pub venue_id: String,
    pub relay_hmac_secret: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    supabase_url: String,
    supabase_key: String,
    venue_id: String,
    relay_hmac_secret: String,
}

fn config_path() -> PathBuf {
    let dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("synalux-print-relay");
    std::fs::create_dir_all(&dir).ok();
    dir.join("config.json")
}

pub fn save_config(cfg: &RelayConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = config_path();
    let json = serde_json::to_string_pretty(cfg)?;
    std::fs::write(&path, &json)?;

    // Also store HMAC secret in OS keychain for extra security
    if let Ok(entry) = keyring::Entry::new("synalux-print-relay", "hmac-secret") {
        let _ = entry.set_password(&cfg.relay_hmac_secret);
    }

    log::info!("Config saved to {}", path.display());
    Ok(())
}

pub fn load_config() -> Result<RelayConfig, Box<dyn std::error::Error + Send + Sync>> {
    let path = config_path();
    let json = std::fs::read_to_string(&path)?;
    let mut cfg: RelayConfig = serde_json::from_str(&json)?;

    // Prefer keychain secret over file
    if let Ok(entry) = keyring::Entry::new("synalux-print-relay", "hmac-secret") {
        if let Ok(secret) = entry.get_password() {
            cfg.relay_hmac_secret = secret;
        }
    }

    Ok(cfg)
}

pub async fn fetch_config_from_token(token: &str) -> Result<RelayConfig, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();

    // The token is a one-time setup token from the POS API
    // POST /api/v1/pos/settings/relay/claim with the token
    let base_url = if token.contains('|') {
        let parts: Vec<&str> = token.splitn(2, '|').collect();
        let url = parts[0].to_string();
        // token is url|actual_token format
        url
    } else {
        "https://pos.synalux.ai".to_string()
    };

    let actual_token = if token.contains('|') {
        token.splitn(2, '|').nth(1).unwrap_or(token).to_string()
    } else {
        token.to_string()
    };

    let resp = client
        .post(format!("{}/api/v1/pos/settings/relay/claim", base_url))
        .json(&serde_json::json!({ "token": actual_token }))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Setup failed: {} — {}", status, body).into());
    }

    let data: TokenResponse = resp.json().await?;

    Ok(RelayConfig {
        supabase_url: data.supabase_url,
        supabase_key: data.supabase_key,
        venue_id: data.venue_id,
        relay_hmac_secret: data.relay_hmac_secret,
    })
}
