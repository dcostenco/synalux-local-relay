use crate::config::RelayConfig;
use crate::printer;
use crate::AppState;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::Arc;

type HmacSha256 = Hmac<Sha256>;

fn deep_sorted_stringify(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
        serde_json::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(deep_sorted_stringify).collect();
            format!("[{}]", items.join(","))
        }
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let pairs: Vec<String> = keys
                .iter()
                .map(|k| format!("\"{}\":{}", k, deep_sorted_stringify(&map[*k])))
                .collect();
            format!("{{{}}}", pairs.join(","))
        }
    }
}

fn verify_hmac(data: &serde_json::Value, secret: &str) -> bool {
    let obj = match data.as_object() {
        Some(o) => o,
        None => return false,
    };

    let sig = match obj.get("_sig").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return false,
    };

    let mut rest = serde_json::Map::new();
    for (k, v) in obj {
        if k != "_sig" {
            rest.insert(k.clone(), v.clone());
        }
    }

    let canonical = deep_sorted_stringify(&serde_json::Value::Object(rest));

    let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(canonical.as_bytes());
    let expected = hex::encode(mac.finalize().into_bytes());

    // Constant-time comparison
    if expected.len() != sig.len() {
        return false;
    }
    expected
        .bytes()
        .zip(sig.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

pub async fn run_relay(cfg: &RelayConfig, state: Arc<AppState>) {
    let channel_name = format!("local-relay:{}", cfg.venue_id);
    log::info!("Subscribing to Supabase channel: {}", channel_name);

    // Supabase Realtime via WebSocket
    let ws_url = cfg.supabase_url
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    let realtime_url = format!(
        "{}/realtime/v1/websocket?apikey={}&vsn=1.0.0",
        ws_url, cfg.supabase_key
    );

    let connect_result = tokio_tungstenite::connect_async(&realtime_url).await;
    let (ws_stream, _) = match connect_result {
        Ok(r) => r,
        Err(e) => {
            log::error!("WebSocket connection failed: {}", e);
            return;
        }
    };

    use futures_util::{SinkExt, StreamExt};
    let (mut write, mut read) = ws_stream.split();

    // Join the channel
    let join_msg = serde_json::json!({
        "topic": format!("realtime:{}", channel_name),
        "event": "phx_join",
        "payload": {},
        "ref": "1"
    });
    if let Err(e) = write.send(tokio_tungstenite::tungstenite::Message::Text(join_msg.to_string())).await {
        log::error!("Failed to join channel: {}", e);
        return;
    }

    log::info!("Successfully subscribed and listening for events!");
    {
        let mut s = state.status.lock().await;
        s.connected = true;
    }

    // Heartbeat task
    let mut write_hb = write;
    let hb_handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
            let hb = serde_json::json!({
                "topic": "phoenix",
                "event": "heartbeat",
                "payload": {},
                "ref": "hb"
            });
            if write_hb.send(tokio_tungstenite::tungstenite::Message::Text(hb.to_string())).await.is_err() {
                break;
            }
        }
    });

    // Process messages
    while let Some(msg) = read.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                log::error!("WebSocket error: {}", e);
                break;
            }
        };

        if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                let event = parsed.get("event").and_then(|e| e.as_str()).unwrap_or("");
                if event == "broadcast" {
                    if let Some(payload) = parsed.get("payload").and_then(|p| p.get("payload")) {
                        let broadcast_event = parsed
                            .get("payload")
                            .and_then(|p| p.get("event"))
                            .and_then(|e| e.as_str())
                            .unwrap_or("");

                        if !verify_hmac(payload, &cfg.relay_hmac_secret) {
                            log::error!("[{}] HMAC verification failed — ignoring message", broadcast_event);
                            continue;
                        }

                        match broadcast_event {
                            "tcp-request" => {
                                let host = payload.get("host").and_then(|v| v.as_str()).unwrap_or("");
                                let port = payload.get("port").and_then(|v| v.as_u64()).unwrap_or(9100) as u16;
                                let b64 = payload.get("bytesBase64").and_then(|v| v.as_str()).unwrap_or("");

                                if let Ok(data) = base64_decode(b64) {
                                    match printer::tcp_print(host, port, &data).await {
                                        Ok(_) => {
                                            let mut s = state.status.lock().await;
                                            s.print_count += 1;
                                        }
                                        Err(e) => log::error!("[tcp-request] Print failed: {}", e),
                                    }
                                } else {
                                    log::error!("[tcp-request] Invalid base64 payload");
                                }
                            }
                            "http-request" => {
                                let url = payload.get("url").and_then(|v| v.as_str()).unwrap_or("");
                                let method = payload.get("method").and_then(|v| v.as_str()).unwrap_or("POST");
                                let headers = payload.get("headers").cloned().unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                                let body = payload.get("body").and_then(|v| v.as_str()).unwrap_or("");

                                match printer::http_print(url, method, &headers, body).await {
                                    Ok(_) => {
                                        let mut s = state.status.lock().await;
                                        s.print_count += 1;
                                    }
                                    Err(e) => log::error!("[http-request] Print failed: {}", e),
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    hb_handle.abort();
    log::warn!("Relay loop ended");
}

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut decoder = base64_reader(input.as_bytes());
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn base64_reader(input: &[u8]) -> impl std::io::Read + '_ {
    struct B64Reader<'a> {
        input: &'a [u8],
        pos: usize,
        buf: [u8; 3],
        buf_len: usize,
        buf_pos: usize,
    }

    impl<'a> std::io::Read for B64Reader<'a> {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let mut written = 0;
            while written < out.len() {
                if self.buf_pos < self.buf_len {
                    out[written] = self.buf[self.buf_pos];
                    self.buf_pos += 1;
                    written += 1;
                    continue;
                }
                // Decode next 4 chars
                let mut quad = [0u8; 4];
                let mut qi = 0;
                while qi < 4 && self.pos < self.input.len() {
                    let c = self.input[self.pos];
                    self.pos += 1;
                    if c == b'\n' || c == b'\r' || c == b' ' { continue; }
                    quad[qi] = c;
                    qi += 1;
                }
                if qi == 0 { break; }
                let vals: Vec<Option<u8>> = quad[..qi].iter().map(|&c| match c {
                    b'A'..=b'Z' => Some(c - b'A'),
                    b'a'..=b'z' => Some(c - b'a' + 26),
                    b'0'..=b'9' => Some(c - b'0' + 52),
                    b'+' => Some(62),
                    b'/' => Some(63),
                    b'=' => None,
                    _ => None,
                }).collect();
                let v: Vec<u8> = vals.iter().filter_map(|v| *v).collect();
                self.buf_len = 0;
                if v.len() >= 2 { self.buf[self.buf_len] = (v[0] << 2) | (v[1] >> 4); self.buf_len += 1; }
                if v.len() >= 3 { self.buf[self.buf_len] = (v[1] << 4) | (v[2] >> 2); self.buf_len += 1; }
                if v.len() >= 4 { self.buf[self.buf_len] = (v[2] << 6) | v[3]; self.buf_len += 1; }
                self.buf_pos = 0;
            }
            Ok(written)
        }
    }

    B64Reader { input, pos: 0, buf: [0; 3], buf_len: 0, buf_pos: 0 }
}
