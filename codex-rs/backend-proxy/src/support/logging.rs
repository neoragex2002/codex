use std::fmt::Write as _;

use reqwest::header::HeaderMap;
use tiny_http::Request;
use serde_json::{json, Value};

const MAX_HEADER_VALUE: usize = 200;
const MAX_BODY_PREVIEW: usize = 4000; // characters

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut out = s.chars().take(max).collect::<String>();
    out.push_str("… (truncated)");
    out
}

fn redact_header_pair(name: &str, value: &str) -> (String, String) {
    let lname = name.to_ascii_lowercase();
    if lname == "authorization" {
        return (name.to_string(), "<redacted>".to_string());
    }
    if lname == "chatgpt-account-id" {
        let redacted = if value.len() > 8 {
            let (head, tail) = value.split_at(value.len() - 4);
            let stars = "*".repeat(head.len());
            format!("{}{}", stars, tail)
        } else {
            "****".to_string()
        };
        return (name.to_string(), redacted);
    }
    (name.to_string(), truncate_str(value, MAX_HEADER_VALUE))
}

fn pretty_body(bytes: &[u8]) -> String {
    // Try JSON pretty first
    if let Ok(val) = serde_json::from_slice::<serde_json::Value>(bytes) {
        let s = serde_json::to_string_pretty(&val)
            .unwrap_or_else(|_| String::from_utf8_lossy(bytes).to_string());
        return truncate_str(&s, MAX_BODY_PREVIEW);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => truncate_str(s, MAX_BODY_PREVIEW),
        Err(_) => {
            // Hex dump small binary bodies
            let mut out = String::new();
            for (i, b) in bytes.iter().enumerate() {
                if i > 0 && i % 16 == 0 {
                    let _ = writeln!(out);
                }
                let _ = write!(out, "{:02x} ", b);
                if i > 1024 {
                    // limit output
                    let _ = write!(out, "... (truncated)");
                    break;
                }
            }
            out
        }
    }
}

pub fn log_inbound_request(req: &Request, body: &[u8]) {
    let mut headers = serde_json::Map::new();
    for h in req.headers() {
        let name = h.field.to_string();
        let (k, v) = redact_header_pair(&name, h.value.as_str());
        headers.insert(k, Value::String(v));
    }
    let preview = pretty_body(body);
    let obj = json!({
        "type": "inbound_request",
        "method": req.method().to_string(),
        "url": req.url(),
        "headers": Value::Object(headers),
        "body_truncated": preview.ends_with("… (truncated)"),
    });
    eprintln!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
    eprintln!("--- inbound body (preview) ---\n{}\n--- end body ---", preview);
}

pub fn log_upstream_request(url: &str, headers: &HeaderMap, body: &[u8]) {
    let mut hmap = serde_json::Map::new();
    for (k, v) in headers.iter() {
        let key = k.as_str();
        let val = v.to_str().unwrap_or("");
        let (k2, v2) = redact_header_pair(key, val);
        hmap.insert(k2, Value::String(v2));
    }
    let preview = pretty_body(body);
    let obj = json!({
        "type": "upstream_request",
        "method": "POST",
        "url": url,
        "headers": Value::Object(hmap),
        "body_truncated": preview.ends_with("… (truncated)"),
    });
    eprintln!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
    eprintln!("--- upstream request body (preview) ---\n{}\n--- end body ---", preview);
}

pub fn log_upstream_response(status: u16, headers: &HeaderMap, body: &[u8]) {
    let mut hmap = serde_json::Map::new();
    for (k, v) in headers.iter() {
        let key = k.as_str();
        let val = v.to_str().unwrap_or("");
        let (k2, v2) = redact_header_pair(key, val);
        hmap.insert(k2, Value::String(v2));
    }
    let preview = pretty_body(body);
    let obj = json!({
        "type": "upstream_response",
        "status": status,
        "headers": Value::Object(hmap),
        "body_truncated": preview.ends_with("… (truncated)"),
    });
    eprintln!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
    eprintln!("--- upstream response body (preview) ---\n{}\n--- end body ---", preview);
}

pub fn log_sse_start(url: &str) {
    let obj = json!({
        "type": "sse_start",
        "url": url,
        "message": "Upstream response is text/event-stream; streaming to client"
    });
    eprintln!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
}
