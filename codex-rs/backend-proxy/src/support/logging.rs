use std::fmt::Write as _;

use reqwest::header::HeaderMap;
use tiny_http::Request;
use serde_json::{json, Value};

const MAX_HEADER_VALUE: usize = 200;
const MAX_BODY_PREVIEW: usize = 4000; // characters (fallback only)
const MAX_JSON_STRING: usize = 800; // per-field truncation limit
const MAX_TEXT_LINE: usize = 1200; // truncate long non-JSON single lines

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

fn truncate_json_value(v: &Value, truncated: &mut bool) -> Value {
    match v {
        Value::String(s) => {
            if s.len() > MAX_JSON_STRING {
                *truncated = true;
                Value::String(truncate_str(s, MAX_JSON_STRING))
            } else {
                Value::String(s.clone())
            }
        }
        Value::Array(arr) => {
            let mut any = false;
            let new = arr.iter().map(|x| {
                let mut t = false;
                let nv = truncate_json_value(x, &mut t);
                any = any || t;
                nv
            }).collect::<Vec<_>>();
            *truncated = *truncated || any;
            Value::Array(new)
        }
        Value::Object(map) => {
            let mut obj = serde_json::Map::new();
            let mut any = false;
            for (k, v2) in map.iter() {
                let mut t = false;
                let nv = truncate_json_value(v2, &mut t);
                any = any || t;
                obj.insert(k.clone(), nv);
            }
            *truncated = *truncated || any;
            Value::Object(obj)
        }
        _ => v.clone(),
    }
}

fn format_sse_with_field_truncation(s: &str) -> (String, bool) {
    // Parse line by line; pretty print JSON inside data: ... lines with per-field truncation.
    let mut out = String::new();
    let mut any_truncated = false;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("data: ") {
            let trimmed = rest.trim();
            if trimmed.starts_with('{') || trimmed.starts_with('[') {
                if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
                    let mut t = false;
                    let val2 = truncate_json_value(&val, &mut t);
                    any_truncated = any_truncated || t;
                    if let Ok(pretty) = serde_json::to_string_pretty(&val2) {
                        let _ = writeln!(out, "data: {}", pretty.replace('\n', "\n"));
                        continue;
                    }
                }
            }
            // Non-JSON data or parse failed; truncate the line
            let _ = writeln!(out, "data: {}", truncate_str(trimmed, MAX_TEXT_LINE));
            any_truncated = true;
        } else if line.starts_with("event:") || line.starts_with(":") || line.is_empty() {
            let _ = writeln!(out, "{}", line);
        } else {
            // Other lines, just limit length for safety
            let _ = writeln!(out, "{}", truncate_str(line, MAX_TEXT_LINE));
            if line.len() > MAX_TEXT_LINE { any_truncated = true; }
        }
    }
    (out, any_truncated)
}

fn pretty_preview(bytes: &[u8]) -> (String, bool) {
    // Try JSON pretty with field-level truncation
    if let Ok(val) = serde_json::from_slice::<serde_json::Value>(bytes) {
        let mut truncated = false;
        let val2 = truncate_json_value(&val, &mut truncated);
        let s = serde_json::to_string_pretty(&val2)
            .unwrap_or_else(|_| String::from_utf8_lossy(bytes).to_string());
        return (s, truncated);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => {
            // Heuristically treat as SSE if it contains lines starting with "event:" or "data:"
            if s.contains("\nevent:") || s.starts_with("event:") || s.contains("\ndata: ") || s.starts_with("data: ") {
                return format_sse_with_field_truncation(s);
            }
            (truncate_str(s, MAX_BODY_PREVIEW), s.len() > MAX_BODY_PREVIEW)
        }
        Err(_) => {
            // Hex dump small binary bodies
            let mut out = String::new();
            let mut truncated = false;
            for (i, b) in bytes.iter().enumerate() {
                if i > 0 && i % 16 == 0 {
                    let _ = writeln!(out);
                }
                let _ = write!(out, "{:02x} ", b);
                if i > 1024 {
                    let _ = write!(out, "... (truncated)");
                    truncated = true;
                    break;
                }
            }
            (out, truncated)
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
    let (preview, truncated) = pretty_preview(body);
    let obj = json!({
        "type": "inbound_request",
        "method": req.method().to_string(),
        "url": req.url(),
        "headers": Value::Object(headers),
        "body_truncated": truncated,
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
    let (preview, truncated) = pretty_preview(body);
    let obj = json!({
        "type": "upstream_request",
        "method": "POST",
        "url": url,
        "headers": Value::Object(hmap),
        "body_truncated": truncated,
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
    let (preview, truncated) = pretty_preview(body);
    let obj = json!({
        "type": "upstream_response",
        "status": status,
        "headers": Value::Object(hmap),
        "body_truncated": truncated,
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
