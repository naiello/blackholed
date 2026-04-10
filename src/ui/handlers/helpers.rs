use std::net::{IpAddr, SocketAddr};

use axum::{extract::ConnectInfo, http::HeaderMap};

const PROXY_IP_HEADERS: &[&str] = &["X-Forwarded-For", "X-Real-Ip"];

/// Extract the client's IP address, considering proxy headers
pub fn extract_client_ip(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: &HeaderMap,
) -> IpAddr {
    PROXY_IP_HEADERS
        .iter()
        .find_map(|h| {
            headers
                .get(*h)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.split(',').next())
                .and_then(|s| s.trim().parse().ok())
        })
        .unwrap_or_else(|| addr.ip())
}

/// Check if this is an HTMX request
pub fn is_htmx_request(headers: &HeaderMap) -> bool {
    headers
        .get("HX-Request")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == "true")
        .unwrap_or(false)
}
