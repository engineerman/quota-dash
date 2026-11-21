mod auth;
mod parse;

pub use auth::{default_auth_path, read_auth, CodexAuth, CodexAuthError};
pub use parse::parse_usage;

use crate::models::{ProviderSnapshot, ProviderStatus};
use chrono::Utc;
use serde_json::Value;

const DEFAULT_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

pub async fn fetch(http: &reqwest::Client, api_base: &str) -> ProviderSnapshot {
    let Some(path) = default_auth_path() else {
        return fail(
            ProviderStatus::NeedsLogin,
            "USERPROFILE/HOME not set; cannot locate Codex auth.",
        );
    };
    if !path.exists() {
        return fail(
            ProviderStatus::NeedsLogin,
            "Run `codex login` once, then refresh.",
        );
    }

    let auth = match read_auth(&path) {
        Ok(a) => a,
        Err(CodexAuthError::NeedsLogin(msg)) => return fail(ProviderStatus::NeedsLogin, &msg),
        Err(CodexAuthError::Error(msg)) => return fail(ProviderStatus::Error, &msg),
    };

    fetch_with_auth(http, api_base, &auth).await
}

pub async fn fetch_with_auth(
    http: &reqwest::Client,
    api_base: &str,
    auth: &CodexAuth,
) -> ProviderSnapshot {
    fetch_with_auth_refresh(http, api_base, DEFAULT_TOKEN_URL, auth).await
}

/// Like [`fetch_with_auth`], but with an injectable OAuth token URL (for tests).
pub async fn fetch_with_auth_refresh(
    http: &reqwest::Client,
    api_base: &str,
    refresh_url: &str,
    auth: &CodexAuth,
) -> ProviderSnapshot {
    match get_usage(http, api_base, auth).await {
        Ok(usage) => {
            let mut snap = parse_usage(&usage);
            snap.fetched_at = Utc::now();
            snap
        }
        Err(ApiError::Unauthorized) => {
            let Some(rt) = auth.refresh_token.as_deref().filter(|s| !s.is_empty()) else {
                return fail(
                    ProviderStatus::NeedsLogin,
                    "Codex session expired. Run `codex login`.",
                );
            };
            let fresh = match refresh_tokens(http, refresh_url, rt).await {
                Ok(t) => t,
                Err(_) => {
                    return fail(
                        ProviderStatus::NeedsLogin,
                        "Codex session expired. Run `codex login`.",
                    );
                }
            };
            // v1: keep refreshed tokens in-memory only (avoid racing Codex CLI on auth.json).
            let mut auth2 = auth.clone();
            auth2.access_token = fresh.access_token;
            if let Some(r) = fresh.refresh_token {
                auth2.refresh_token = Some(r);
            }
            match get_usage(http, api_base, &auth2).await {
                Ok(usage) => {
                    let mut snap = parse_usage(&usage);
                    snap.fetched_at = Utc::now();
                    snap
                }
                Err(ApiError::Unauthorized) => fail(
                    ProviderStatus::NeedsLogin,
                    "Codex session expired. Run `codex login`.",
                ),
                Err(ApiError::Other(msg)) => fail(ProviderStatus::Error, &msg),
            }
        }
        Err(ApiError::Other(msg)) => fail(ProviderStatus::Error, &msg),
    }
}

struct RefreshedTokens {
    access_token: String,
    refresh_token: Option<String>,
}

enum ApiError {
    Unauthorized,
    Other(String),
}

async fn get_usage(
    http: &reqwest::Client,
    api_base: &str,
    auth: &CodexAuth,
) -> Result<Value, ApiError> {
    let url = format!("{}/backend-api/wham/usage", api_base.trim_end_matches('/'));
    let mut req = http.get(&url).bearer_auth(&auth.access_token);
    if let Some(acc) = auth.account_id.as_deref().filter(|s| !s.is_empty()) {
        req = req.header("ChatGPT-Account-Id", acc);
    }
    let res = req
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    let status = res.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(ApiError::Unauthorized);
    }
    if !status.is_success() {
        return Err(ApiError::Other(format!("Codex usage HTTP {status}")));
    }
    res.json::<Value>()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))
}

async fn refresh_tokens(
    http: &reqwest::Client,
    refresh_url: &str,
    refresh_token: &str,
) -> Result<RefreshedTokens, ApiError> {
    let body = serde_json::json!({
        "client_id": CLIENT_ID,
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "scope": "openid profile email"
    });
    let res = http
        .post(refresh_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    let status = res.status();
    if !status.is_success() {
        return Err(ApiError::Other(format!("Codex refresh HTTP {status}")));
    }
    let v: Value = res
        .json()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    let access_token = v
        .get("access_token")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ApiError::Other("Codex refresh response missing access_token".into()))?;
    let refresh_token = v
        .get("refresh_token")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok(RefreshedTokens {
        access_token,
        refresh_token,
    })
}

fn fail(status: ProviderStatus, message: &str) -> ProviderSnapshot {
    ProviderSnapshot {
        id: "codex".into(),
        kind: "codex".into(),
        display_name: "Codex".into(),
        account: None,
        plan: None,
        status,
        message: Some(message.to_string()),
        windows: Vec::new(),
        fetched_at: Utc::now(),
    }
}
