use anyhow::Result;
use reqwest::header::AUTHORIZATION;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderName;
use reqwest::header::HeaderValue;
use url::Url;

use super::auth_loader::AuthContext;

pub fn build_upstream_headers(
    runtime: &tokio::runtime::Runtime,
    auth_ctx: &AuthContext,
) -> Result<HeaderMap> {
    let (token, maybe_account_id) = auth_ctx.bearer_and_optional_account_id(runtime)?;
    let mut headers = HeaderMap::new();

    let mut auth_header_value = HeaderValue::from_str(&format!("Bearer {token}"))?;
    auth_header_value.set_sensitive(true);
    headers.insert(AUTHORIZATION, auth_header_value);

    if let Some(account_id) = maybe_account_id
        && let Ok(name) = HeaderName::from_bytes(b"ChatGPT-Account-Id")
    {
        let hv = HeaderValue::from_str(&account_id)?;
        headers.insert(name, hv);
    }

    Ok(headers)
}

pub fn host_header_for(url: &str) -> Option<HeaderValue> {
    Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(std::string::ToString::to_string))
        .and_then(|host| HeaderValue::from_str(&host).ok())
}
