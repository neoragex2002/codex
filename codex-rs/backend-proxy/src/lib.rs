use std::net::SocketAddr;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use clap::Parser;
use reqwest::blocking::Client;
use tiny_http::Method;
use tiny_http::Request;
use tiny_http::Response;
use tiny_http::Server;
use tiny_http::StatusCode;

mod support;
use support::auth_loader::AuthContext;
use support::headers::build_upstream_headers;
use support::logging::{log_inbound_request, log_upstream_request, log_upstream_response, log_sse_start};
use support::router::Router;
use support::translate::translate_openai_responses_to_codex;
use support::utils::write_server_info;

/// CLI arguments for the backend proxy.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "codex-backend-proxy",
    about = "OpenAI-compatible proxy to ChatGPT backend"
)]
pub struct Args {
    /// Port to listen on. If not set, an ephemeral port is used.
    #[arg(long)]
    pub port: Option<u16>,

    /// Path to a JSON file to write startup info (single line). Includes {"port": <u16>, "pid": <u32>}.
    #[arg(long, value_name = "FILE")]
    pub server_info: Option<PathBuf>,

    /// Enable HTTP shutdown endpoint at GET /shutdown
    #[arg(long)]
    pub http_shutdown: bool,

    /// Override the default base url for ChatGPT backend (default: https://chatgpt.com/backend-api/codex)
    #[arg(long, value_name = "URL")]
    pub base_url: Option<String>,

    /// Override codex home directory used to read auth.json (default: ~/.codex)
    #[arg(long, value_name = "PATH")]
    pub codex_home: Option<PathBuf>,

    /// Print verbose proxy logs (routing, retries, contexts). No secrets are logged.
    #[arg(long)]
    pub verbose: bool,

    /// Bind address to listen on. Default: 127.0.0.1. Use 0.0.0.0 for LAN/mirrored networking.
    #[arg(long, default_value = "127.0.0.1", value_name = "ADDR")]
    pub bind: String,
}

pub fn run_main(args: Args) -> Result<()> {
    let (listener, bound_addr) = bind_listener(&args.bind, args.port)?;
    if let Some(path) = args.server_info.as_ref() {
        write_server_info(path, bound_addr.port())?;
    }
    let server = Server::from_listener(listener, None)
        .map_err(|err| anyhow!("creating HTTP server: {err}"))?;

    let client = Arc::new(
        Client::builder()
            .timeout(None::<Duration>)
            .build()
            .context("building reqwest client")?,
    );

    let runtime = Arc::new(tokio::runtime::Runtime::new().context("creating tokio runtime")?);

    // Load auth context once; operations that require async are bridged via runtime.
    let auth_ctx = Arc::new(AuthContext::new(args.codex_home.clone())?);

    let base_url = args
        .base_url
        .unwrap_or_else(|| "https://chatgpt.com/backend-api/codex".to_string());
    let router = Arc::new(Router::new(&base_url)?);

    eprintln!("codex-backend-proxy listening on {bound_addr}; base_url={base_url}");

    let http_shutdown = args.http_shutdown;
    let verbose = args.verbose;
    for request in server.incoming_requests() {
        let client = client.clone();
        let runtime = runtime.clone();
        let auth_ctx = auth_ctx.clone();
        let router = router.clone();
        std::thread::spawn(move || {
            if http_shutdown && request.method() == &Method::Get && request.url() == "/shutdown" {
                let _ = request.respond(Response::new_empty(StatusCode(200)));
                std::process::exit(0);
            }

            if let Err(e) = handle_request(&client, &runtime, &auth_ctx, &router, verbose, request)
            {
                if verbose {
                    eprintln!("proxy error: {e:#}");
                } else {
                    eprintln!("proxy error: {e}");
                }
            }
        });
    }

    Err(anyhow!("server stopped unexpectedly"))
}

fn bind_listener(bind: &str, port: Option<u16>) -> Result<(TcpListener, SocketAddr)> {
    use std::net::{IpAddr, Ipv4Addr};
    let ip: IpAddr = bind
        .parse()
        .unwrap_or(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
    let addr = SocketAddr::from((ip, port.unwrap_or(0)));
    let listener = TcpListener::bind(addr).with_context(|| format!("failed to bind {addr}"))?;
    let bound = listener.local_addr().context("failed to read local_addr")?;
    Ok((listener, bound))
}

fn handle_request(
    client: &Client,
    runtime: &tokio::runtime::Runtime,
    auth_ctx: &AuthContext,
    router: &Router,
    verbose: bool,
    mut req: Request,
) -> Result<()> {
    // Health
    if req.method() == &Method::Get && req.url() == "/health" {
        let body = serde_json::json!({"status": "ok", "version": env!("CARGO_PKG_VERSION")});
        let bytes = serde_json::to_vec(&body)?;
        let mut resp = Response::from_data(bytes);
        if let Ok(h) = tiny_http::Header::from_bytes(b"Content-Type", b"application/json") {
            resp.add_header(h);
        }
        let _ = req.respond(resp);
        return Ok(());
    }

    let method = req.method().clone();
    let url_path = req.url().to_string();
    let Some(route) = router.match_route(&method, &url_path) else {
        let resp = Response::new_empty(StatusCode(403));
        let _ = req.respond(resp);
        return Ok(());
    };

    // Basic routing log to aid debugging (no sensitive data).
    let method_dbg = format!("{method:?}");
    if verbose {
        eprintln!(
            "proxy route: {method_dbg} {url_path} -> {}",
            route.upstream_url
        );
    }

    // Read request body
    let mut body = Vec::new();
    let mut reader = req.as_reader();
    std::io::Read::read_to_end(&mut reader, &mut body)?;
    if verbose {
        log_inbound_request(&req, &body);
    }

    // Build upstream headers (may use async CodexAuth ops)
    let mut headers =
        build_upstream_headers(runtime, auth_ctx).context("building upstream headers")?;
    // Ensure Host header matches upstream domain.
    if let Some(host) = support::headers::host_header_for(&route.upstream_url) {
        headers.insert(reqwest::header::HOST, host);
    }

    // Forward selected incoming headers (excluding hop-by-hop, sensitive, and length-related)
    for header in req.headers() {
        let name_ascii = header.field.as_str();
        let lower = name_ascii.to_ascii_lowercase();
        if lower.as_str() == "authorization" || lower.as_str() == "host" {
            continue;
        }
        if matches!(
            lower.as_str(),
            "connection"
                | "keep-alive"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
                | "content-length"
        ) {
            continue;
        }

        let header_name = match reqwest::header::HeaderName::from_bytes(lower.as_bytes()) {
            Ok(name) => name,
            Err(_) => continue,
        };
        if let Ok(value) = reqwest::header::HeaderValue::from_bytes(header.value.as_bytes()) {
            headers.append(header_name, value);
        }
    }

    // Translate request for Codex backend (Responses API)
    // Force official instructions and optionally insert system as input_texts
    let mut upstream_body = body.clone();
    let mut is_stream = false;
    if !body.is_empty() {
        if let Ok(json_val) = serde_json::from_slice::<serde_json::Value>(&body) {
            if let Ok((new_v, stream_flag, _modified)) =
                translate_openai_responses_to_codex(json_val)
            {
                upstream_body = serde_json::to_vec(&new_v)?;
                is_stream = stream_flag;
            }
        }
    }

    // Enforce required upstream headers
    // - OpenAI-Beta: responses=experimental
    if let Ok(name) = reqwest::header::HeaderName::from_bytes(b"OpenAI-Beta") {
        let value = reqwest::header::HeaderValue::from_static("responses=experimental");
        headers.insert(name, value);
    }
    // - Accept: text/event-stream if stream=true
    if is_stream {
        headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("text/event-stream"),
        );
    }

    if verbose {
        log_upstream_request(&route.upstream_url, &headers, &upstream_body);
    }

    // Upstream request
    let upstream_resp = client
        .post(route.upstream_url.as_str())
        .headers(headers.clone())
        .body(upstream_body.clone())
        .send()
        .with_context(|| {
            format!(
                "forwarding {method_dbg} {url_path} to {}",
                route.upstream_url
            )
        })?;

    if verbose {
        let sc = upstream_resp.status();
        eprintln!(
            "upstream status: {} for {} {}",
            sc, method_dbg, route.upstream_url
        );
    }

    if upstream_resp.status().as_u16() == 401 {
        // Attempt one refresh and retry once.
        if verbose {
            eprintln!("upstream 401: attempting token refresh");
        }
        if let Err(err) = auth_ctx.try_refresh(runtime) {
            if verbose {
                eprintln!("token refresh failed: {err:#}");
            } else {
                eprintln!("token refresh failed: {err}");
            }
        } else {
            headers = build_upstream_headers(runtime, auth_ctx)
                .context("rebuilding headers after refresh")?;
            if let Some(host) = support::headers::host_header_for(&route.upstream_url) {
                headers.insert(reqwest::header::HOST, host);
            }
            if let Ok(name) = reqwest::header::HeaderName::from_bytes(b"OpenAI-Beta") {
                let value = reqwest::header::HeaderValue::from_static("responses=experimental");
                headers.insert(name, value);
            }
            if is_stream {
                headers.insert(
                    reqwest::header::ACCEPT,
                    reqwest::header::HeaderValue::from_static("text/event-stream"),
                );
            }
            let retry = client
                .post(route.upstream_url.as_str())
                .headers(headers)
                .body(upstream_body)
                .send()
                .with_context(|| {
                    format!(
                        "forwarding (retry) {method_dbg} {url_path} to {}",
                        route.upstream_url
                    )
                })?;
            if verbose {
                let sc = retry.status();
                eprintln!(
                    "upstream status (retry): {} for {} {}",
                    sc, method_dbg, route.upstream_url
                );
            }
            return respond_stream(req, retry, verbose);
        }
    }

    respond_stream(req, upstream_resp, verbose)
}

struct Tee<R: std::io::Read> {
    inner: R,
    buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
}

impl<R: std::io::Read> std::io::Read for Tee<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(out)?;
        if n > 0 {
            if let Ok(mut guard) = self.buf.lock() {
                guard.extend_from_slice(&out[..n]);
            }
        }
        Ok(n)
    }
}

fn respond_stream(
    req: Request,
    upstream_resp: reqwest::blocking::Response,
    verbose: bool,
) -> Result<()> {
    let status = upstream_resp.status();
    let headers_for_log = upstream_resp.headers().clone();
    if verbose {
        if let Some(ct) = headers_for_log
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
        {
            if ct.to_ascii_lowercase().contains("text/event-stream") {
                log_sse_start("upstream");
            }
        }
    }
    let mut response_headers = Vec::new();
    for (name, value) in upstream_resp.headers().iter() {
        // Skip headers that tiny_http manages itself.
        if matches!(
            name.as_str(),
            "content-length" | "transfer-encoding" | "connection" | "trailer" | "upgrade"
        ) {
            continue;
        }
        if let Ok(header) =
            tiny_http::Header::from_bytes(name.as_str().as_bytes(), value.as_bytes())
        {
            response_headers.push(header);
        }
    }

    let content_length = upstream_resp.content_length().and_then(|len| {
        if len <= usize::MAX as u64 {
            Some(len as usize)
        } else {
            None
        }
    });

    // Prepare tee reader to capture the upstream body while streaming to client
    let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let tee = Tee {
        inner: upstream_resp,
        buf: buf.clone(),
    };

    let response = Response::new(
        StatusCode(status.as_u16()),
        response_headers,
        tee,
        content_length,
        None,
    );

    let _ = req.respond(response);

    if verbose {
        // Best-effort capture of response headers and body for logging
        let logged_body = buf.lock().map(|v| v.clone()).unwrap_or_default();
        log_upstream_response(status.as_u16(), &headers_for_log, &logged_body);
    }
    Ok(())
}
