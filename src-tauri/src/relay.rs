use crate::config::RelayConfig;
use crate::printer;
use crate::AppState;
use std::sync::Arc;

pub async fn run_relay(cfg: &RelayConfig, state: Arc<AppState>) {
    log::info!("Starting DB print queue relay for venue {}", cfg.venue_id);

    let client = reqwest::Client::new();
    let rest_url = format!("{}/rest/v1/rpc/claim_print_job", cfg.supabase_url);
    let complete_url = format!("{}/rest/v1/rpc/complete_print_job", cfg.supabase_url);

    {
        let mut s = state.status.lock().await;
        s.connected = true;
    }

    log::info!("Successfully connected — polling for print jobs");

    loop {
        match claim_and_process(&client, &rest_url, &complete_url, cfg, &state).await {
            Ok(true) => {
                // Processed a job — immediately check for more
                continue;
            }
            Ok(false) => {
                // No jobs — wait 2 seconds before polling again
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            }
            Err(e) => {
                log::error!("Poll error: {}", e);
                {
                    let mut s = state.status.lock().await;
                    s.connected = false;
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                {
                    let mut s = state.status.lock().await;
                    s.connected = true;
                }
            }
        }
    }
}

async fn claim_and_process(
    client: &reqwest::Client,
    claim_url: &str,
    complete_url: &str,
    cfg: &RelayConfig,
    state: &Arc<AppState>,
) -> Result<bool, String> {
    // Claim a pending job atomically
    let resp = client
        .post(claim_url)
        .header("apikey", &cfg.supabase_key)
        .header("Authorization", format!("Bearer {}", cfg.supabase_key))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({ "p_venue_id": cfg.venue_id }))
        .send()
        .await
        .map_err(|e| format!("HTTP error: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if body.contains("no rows") || body.is_empty() {
            return Ok(false);
        }
        return Err(format!("Claim failed: {} — {}", status, body));
    }

    let jobs: Vec<serde_json::Value> = resp
        .json()
        .await
        .map_err(|e| format!("Parse error: {}", e))?;

    let job = match jobs.first() {
        Some(j) => j,
        None => return Ok(false),
    };

    let job_id = job.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let printer_ip = job.get("printer_ip").and_then(|v| v.as_str()).unwrap_or("");
    let printer_type = job.get("printer_type").and_then(|v| v.as_str()).unwrap_or("generic");
    let payload_b64 = job.get("payload_base64").and_then(|v| v.as_str()).unwrap_or("");

    if job_id.is_empty() {
        return Ok(false);
    }

    log::info!("[job {}] Printing to {} ({})", &job_id[..8.min(job_id.len())], printer_ip, printer_type);

    let result = if printer_type == "generic" || job.get("http_url").is_none() {
        // TCP print
        match base64_decode(payload_b64) {
            Ok(data) => printer::tcp_print(printer_ip, 9100, &data).await,
            Err(e) => Err(format!("Base64 decode: {}", e)),
        }
    } else {
        // HTTP print (Star/Epson)
        let url = job.get("http_url").and_then(|v| v.as_str()).unwrap_or("");
        let method = job.get("http_method").and_then(|v| v.as_str()).unwrap_or("POST");
        let headers = job.get("http_headers").cloned().unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
        let body = job.get("http_body").and_then(|v| v.as_str()).unwrap_or("");
        printer::http_print(url, method, &headers, body).await
    };

    // Report result
    let (success, error) = match &result {
        Ok(_) => (true, None),
        Err(e) => (false, Some(e.as_str())),
    };

    let _ = client
        .post(complete_url)
        .header("apikey", &cfg.supabase_key)
        .header("Authorization", format!("Bearer {}", cfg.supabase_key))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "p_job_id": job_id,
            "p_success": success,
            "p_error": error,
        }))
        .send()
        .await;

    if success {
        let mut s = state.status.lock().await;
        s.print_count += 1;
    }

    Ok(true)
}

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(input)
        .map_err(|e| e.to_string())
}
