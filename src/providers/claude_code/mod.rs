mod auth;
mod parse;

pub use auth::{
    credentials_path, parse_credentials_json, read_credentials, refresh_oauth, ClaudeOAuth,
    CredError,
};
pub use parse::{parse_usage, plan_name};

use std::path::Path;

use crate::models::{ProviderSnapshot, ProviderStatus};
use chrono::Utc;
use serde_json::Value;

const BETA_HEADER: &str = "oauth-2025-04-20";

/// Discover Claude Code credentials and fetch live OAuth usage.
pub async fn fetch(http: &reqwest::Client, api_base: &str, oauth_base: &str) -> ProviderSnapshot {
    let Some(path) = credentials_path() else {
        return fail(
            ProviderStatus::NeedsLogin,
            "Could not resolve home directory for Claude Code credentials.",
        );
    };
    let oauth = match read_credentials(&path) {
        Ok(o) => o,
        Err(CredError::NeedsLogin(msg)) => return fail(ProviderStatus::NeedsLogin, &msg),
        Err(CredError::Error(msg)) => return fail(ProviderStatus::Error, &msg),
    };
    fetch_with_auth(http, api_base, oauth_base, &oauth, Some(path.as_path())).await
}

/// Fetch usage with an already-loaded OAuth session (injectable bases for tests).
pub async fn fetch_with_auth(
    http: &reqwest::Client,
    api_base: &str,
    oauth_base: &str,
    oauth: &ClaudeOAuth,
    credentials_path: Option<&Path>,
) -> ProviderSnapshot {
    let oauth = if oauth.is_expired_soon() {
        match refresh_oauth(http, oauth_base, oauth, credentials_path).await {
            Ok(o) => o,
            Err(CredError::NeedsLogin(msg)) => return fail(ProviderStatus::NeedsLogin, &msg),
            Err(CredError::Error(msg)) => return fail(ProviderStatus::Error, &msg),
        }
    } else {
        oauth.clone()
    };

    if oauth.access_token.is_empty() {
        return fail(
            ProviderStatus::NeedsLogin,
            "Claude Code has no access token. Run `claude` and /login.",
        );
    }

    let usage = match get_usage(http, api_base, &oauth.access_token).await {
        Ok(v) => v,
        Err(ApiError::Unauthorized) => {
            // One refresh retry if the server rejects a not-yet-expired token.
            if oauth.refresh_token.is_some() {
                let refreshed =
                    match refresh_oauth(http, oauth_base, &oauth, credentials_path).await {
                        Ok(o) => o,
                        Err(CredError::NeedsLogin(msg)) => {
                            return fail(ProviderStatus::NeedsLogin, &msg)
                        }
                        Err(CredError::Error(msg)) => return fail(ProviderStatus::Error, &msg),
                    };
                match get_usage(http, api_base, &refreshed.access_token).await {
                    Ok(v) => v,
                    Err(ApiError::Unauthorized) => {
                        return fail(
                            ProviderStatus::NeedsLogin,
                            "Claude Code session rejected. Run `claude` and /login.",
                        );
                    }
                    Err(ApiError::Other(msg)) => return fail(ProviderStatus::Error, &msg),
                }
            } else {
                return fail(
                    ProviderStatus::NeedsLogin,
                    "Claude Code session rejected. Run `claude` and /login.",
                );
            }
        }
        Err(ApiError::Other(msg)) => return fail(ProviderStatus::Error, &msg),
    };

    let plan = plan_name(
        oauth.rate_limit_tier.as_deref(),
        oauth.subscription_type.as_deref(),
    );
    let mut snap = parse_usage(&usage, plan.as_deref());
    snap.fetched_at = Utc::now();
    if snap.status == ProviderStatus::Ok {
        let src = "Source: Claude Code login";
        snap.message = match snap.message.take() {
            Some(m) => Some(format!("{m}; {src}")),
            None => Some(src.into()),
        };
    }
    snap
}

enum ApiError {
    Unauthorized,
    Other(String),
}

async fn get_usage(
    http: &reqwest::Client,
    api_base: &str,
    access_token: &str,
) -> Result<Value, ApiError> {
    let url = format!("{}/api/oauth/usage", api_base.trim_end_matches('/'));
    let res = http
        .get(&url)
        .bearer_auth(access_token)
        .header("anthropic-beta", BETA_HEADER)
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    let status = res.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(ApiError::Unauthorized);
    }
    if !status.is_success() {
        return Err(ApiError::Other(format!("Claude Code API HTTP {status}")));
    }
    res.json::<Value>()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))
}

fn fail(status: ProviderStatus, message: &str) -> ProviderSnapshot {
    ProviderSnapshot {
        id: "claude-code".into(),
        kind: "claude-code".into(),
        display_name: "Claude Code".into(),
        account: None,
        plan: None,
        status,
        message: Some(message.to_string()),
        windows: Vec::new(),
        fetched_at: Utc::now(),
    }
}
