use std::fmt::Write as _;

use reqwest::header::HeaderMap;
use tiny_http::Request;

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
    (name.to_string(), value.to_string())
}

fn pretty_body(bytes: &[u8]) -> String {
    // Try JSON pretty first
    if let Ok(val) = serde_json::from_slice::<serde_json::Value>(bytes) {
        return serde_json::to_string_pretty(&val)
            .unwrap_or_else(|_| String::from_utf8_lossy(bytes).to_string());
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
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
    eprintln!("================ Inbound Request ================");
    eprintln!("{} {}", req.method(), req.url());
    eprintln!("-- headers:");
    for h in req.headers() {
        let name = h.field.to_string();
        let (k, v) = redact_header_pair(&name, h.value.as_str());
        eprintln!("{}: {}", k, v);
    }
    eprintln!("-- body:");
    eprintln!("{}", pretty_body(body));
    eprintln!("=================================================");
}

pub fn log_upstream_request(url: &str, headers: &HeaderMap, body: &[u8]) {
    eprintln!("================ Upstream Request ===============");
    eprintln!("POST {}", url);
    eprintln!("-- headers:");
    for (k, v) in headers.iter() {
        let key = k.as_str();
        let val = v.to_str().unwrap_or("");
        let (k2, v2) = redact_header_pair(key, val);
        eprintln!("{}: {}", k2, v2);
    }
    eprintln!("-- body:");
    eprintln!("{}", pretty_body(body));
    eprintln!("=================================================");
}

pub fn log_upstream_response(status: u16, headers: &HeaderMap, body: &[u8]) {
    eprintln!("================ Upstream Response ==============");
    eprintln!("status: {}", status);
    eprintln!("-- headers:");
    for (k, v) in headers.iter() {
        let key = k.as_str();
        let val = v.to_str().unwrap_or("");
        let (k2, v2) = redact_header_pair(key, val);
        eprintln!("{}: {}", k2, v2);
    }
    eprintln!("-- body:");
    eprintln!("{}", pretty_body(body));
    eprintln!("=================================================");
}
