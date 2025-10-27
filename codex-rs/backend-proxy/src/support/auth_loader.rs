use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use base64::Engine;
use codex_core::auth::CodexAuth;

#[derive(Clone)]
pub struct AuthContext {
    codex_home: Option<PathBuf>,
    // Lazily loaded; keep latest in memory for cheap re-use.
    auth: Arc<std::sync::Mutex<Option<CodexAuth>>>,
}

impl AuthContext {
    pub fn new(codex_home: Option<PathBuf>) -> Result<Self> {
        Ok(Self {
            codex_home,
            auth: Arc::new(std::sync::Mutex::new(None)),
        })
    }

    fn codex_home_path(&self) -> PathBuf {
        if let Some(p) = &self.codex_home {
            return p.clone();
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join(".codex")
    }

    pub fn load_or_get(&self) -> Result<CodexAuth> {
        if let Ok(guard) = self.auth.lock()
            && let Some(existing) = guard.as_ref()
        {
            return Ok(existing.clone());
        }
        let codex_home = self.codex_home_path();
        let loaded = codex_core::auth::CodexAuth::from_codex_home(&codex_home)
            .context("reading ~/.codex/auth.json; please login with codex login")?
            .ok_or_else(|| anyhow::anyhow!("auth.json not found; please login with codex login"))?;
        if let Ok(mut guard) = self.auth.lock() {
            *guard = Some(loaded.clone());
        }
        Ok(loaded)
    }

    pub fn try_refresh(&self, runtime: &tokio::runtime::Runtime) -> Result<()> {
        let auth = self.load_or_get()?;
        let _new_access = runtime
            .block_on(async move { auth.refresh_token().await })
            .context("refresh_token failed")?;
        Ok(())
    }

    // Note: bearer_and_account_id removed in favor of bearer_and_optional_account_id

    /// Returns bearer token and optional ChatGPT account_id. If running with an
    /// API key (non-ChatGPT), the account_id will be None.
    pub fn bearer_and_optional_account_id(
        &self,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<(String, Option<String>)> {
        let auth = self.load_or_get()?;
        let auth_for_token = auth.clone();
        let token = runtime
            .block_on(async move { auth_for_token.get_token().await })
            .context("get_token failed")?;

        let mut account_id = auth.get_account_id();
        if account_id.is_none()
            && let Ok(td) = runtime.block_on(async { auth.get_token_data().await })
            && let Some(parsed) = extract_account_id_from_id_token(&td.id_token.raw_jwt)
        {
            account_id = Some(parsed);
        }

        Ok((token, account_id))
    }

    /// Heuristic: treat as ChatGPT auth when an account_id is present in the
    /// cached auth.json data.
    pub fn is_chatgpt(&self) -> bool {
        self.load_or_get()
            .ok()
            .and_then(|a| a.get_account_id())
            .is_some()
    }
}

fn extract_account_id_from_id_token(id_token: &str) -> Option<String> {
    // JWT format: header.payload.signature
    let mut parts = id_token.split('.');
    let payload_b64 = match (parts.next(), parts.next(), parts.next()) {
        (Some(_h), Some(p), Some(_s)) if !p.is_empty() => p,
        _ => return None,
    };
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64.as_bytes())
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("https://api.openai.com/auth")
        .and_then(|a| a.get("chatgpt_account_id"))
        .and_then(|s| s.as_str())
        .map(std::string::ToString::to_string)
}
