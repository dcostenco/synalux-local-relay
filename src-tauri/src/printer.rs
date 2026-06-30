use std::net::SocketAddr;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::time::{timeout, Duration};

const TCP_TIMEOUT: Duration = Duration::from_secs(3);
const HTTP_TIMEOUT: Duration = Duration::from_secs(3);

const PRIVATE_HOST_PREFIXES: &[&str] = &["192.168.", "10.", "172.16.", "172.17.", "172.18.",
    "172.19.", "172.20.", "172.21.", "172.22.", "172.23.", "172.24.", "172.25.",
    "172.26.", "172.27.", "172.28.", "172.29.", "172.30.", "172.31."];

const BLOCKED_HOSTS: &[&str] = &["127.0.0.1", "0.0.0.0", "localhost", "::1"];

fn is_allowed_host(host: &str) -> bool {
    if BLOCKED_HOSTS.contains(&host) {
        return false;
    }
    PRIVATE_HOST_PREFIXES.iter().any(|p| host.starts_with(p))
}

const ALLOWED_TCP_PORTS: &[u16] = &[9100, 6101, 515, 9101];

pub async fn tcp_print(host: &str, port: u16, data: &[u8]) -> Result<(), String> {
    if !is_allowed_host(host) {
        return Err(format!("Host {} not allowed", host));
    }
    if !ALLOWED_TCP_PORTS.contains(&port) {
        return Err(format!("Port {} not allowed", port));
    }

    let addr: SocketAddr = format!("{}:{}", host, port)
        .parse()
        .map_err(|e| format!("Invalid address: {}", e))?;

    let mut stream = timeout(TCP_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| "Connection timeout".to_string())?
        .map_err(|e| format!("Connection failed: {}", e))?;

    timeout(TCP_TIMEOUT, stream.write_all(data))
        .await
        .map_err(|_| "Write timeout".to_string())?
        .map_err(|e| format!("Write failed: {}", e))?;

    stream.shutdown().await.ok();

    log::info!("[tcp] Sent {} bytes to {}:{}", data.len(), host, port);
    Ok(())
}

pub async fn http_print(url: &str, method: &str, headers: &serde_json::Value, body: &str) -> Result<(), String> {
    // Validate URL is to a private host
    let parsed = url::Url::parse(url).map_err(|e| format!("Invalid URL: {}", e))?;
    let host = parsed.host_str().unwrap_or("");
    if !is_allowed_host(host) {
        return Err(format!("Host {} not allowed", host));
    }
    if parsed.scheme() != "http" {
        return Err("Only http:// allowed for local printers".to_string());
    }

    let client = reqwest::Client::new();
    let mut req = match method.to_uppercase().as_str() {
        "POST" => client.post(url),
        "GET" => client.get(url),
        _ => return Err(format!("Unsupported method: {}", method)),
    };

    if let Some(map) = headers.as_object() {
        for (k, v) in map {
            if let Some(val) = v.as_str() {
                req = req.header(k.as_str(), val);
            }
        }
    }

    req = req.body(body.to_string());

    let resp = timeout(HTTP_TIMEOUT, req.send())
        .await
        .map_err(|_| "HTTP timeout".to_string())?
        .map_err(|e| format!("HTTP error: {}", e))?;

    let status = resp.status();
    if status.is_success() {
        log::info!("[http] Success: {} → {}", url, status);
        Ok(())
    } else {
        Err(format!("HTTP {}", status))
    }
}
