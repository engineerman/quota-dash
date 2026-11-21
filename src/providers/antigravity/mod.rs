pub mod accounts;
mod auth;
pub mod login;
mod parse;

pub use accounts::{
    list as list_accounts, prepare_add_account, remove as remove_account, switch, track_active,
};
pub use auth::{
    access_token_fresh, ensure_access_token, fresh_t3_auth, parse_cred_blob, read_active_auth,
    revive_from_installed_t3, AntigravityAuth, AntigravityAuthError, CRED_TARGET,
};
pub use login::{find_agy, PendingLogin};
pub use parse::{parse_models, parse_quota_summary};

use crate::models::{ProviderSnapshot, ProviderStatus};
use chrono::Utc;
use futures::future::join_all;
use serde_json::Value;

pub const DEFAULT_API_BASE: &str = "https://cloudcode-pa.googleapis.com/v1internal";

/// Discover WinCred auth and fetch Antigravity quota (production path, single active).
pub async fn fetch(http: &reqwest::Client, api_base: &str) -> ProviderSnapshot {
    fetch_all(http, api_base)
        .await
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            fail(
                None,
                ProviderStatus::NeedsLogin,
                "Sign in with Antigravity once, then refresh.",
            )
        })
}

/// One snapshot per **tracked** account. Empty index → empty vec (UI shows + hesap).
pub async fn fetch_all(http: &reqwest::Client, api_base: &str) -> Vec<ProviderSnapshot> {
    let accounts = accounts::list();
    if accounts.is_empty() {
        return Vec::new();
    }
    let futs = accounts.into_iter().map(|a| {
        let http = http.clone();
        let base = api_base.to_string();
        async move { fetch_one_account(&http, &base, &a.email).await }
    });
    join_all(futs).await
}

async fn fetch_one_account(
    http: &reqwest::Client,
    api_base: &str,
    email: &str,
) -> ProviderSnapshot {
    let auth = match accounts::auth_for(email) {
        Ok(a) => revive_from_installed_t3(a),
        Err(AntigravityAuthError::NeedsLogin(msg)) => match fresh_t3_auth(email) {
            Some(a) => a,
            None => return fail(Some(email), ProviderStatus::NeedsLogin, &msg),
        },
        Err(AntigravityAuthError::Error(msg)) => {
            return fail(Some(email), ProviderStatus::Error, &msg);
        }
    };

    let auth = match ensure_access_token(http, &auth).await {
        Ok(a) => a,
        Err(AntigravityAuthError::NeedsLogin(msg)) => {
            return fail(Some(email), ProviderStatus::NeedsLogin, &msg);
        }
        Err(AntigravityAuthError::Error(msg)) => {
            return fail(Some(email), ProviderStatus::Error, &msg);
        }
    };

    // Persist refreshed token back into vault when possible.
    if let Some(ref em) = auth.email {
        if let Ok(blob) = serde_json::to_string(&serde_json::json!({
            "id_token_email": em,
            "auth_method": auth.auth_method,
            "token": {
                "access_token": auth.access_token,
                "refresh_token": auth.refresh_token,
                "expiry": auth.expiry.map(|t| t.to_rfc3339()),
            }
        })) {
            let _ = accounts::save_vault_json(em, &blob);
        }
    }

    fetch_with_auth(http, api_base, &auth).await
}

/// Testable fetch with injected auth (no WinCred). Same shape as Cursor's seam.
pub async fn fetch_with_auth(
    http: &reqwest::Client,
    api_base: &str,
    auth: &AntigravityAuth,
) -> ProviderSnapshot {
    let summary = fetch_summary_windows(http, api_base, auth).await;
    if !summary.is_empty() {
        return ProviderSnapshot {
            id: snapshot_id(auth.email.as_deref()),
            kind: "antigravity".into(),
            display_name: "Antigravity".into(),
            account: auth.email.clone(),
            plan: None,
            status: ProviderStatus::Ok,
            message: None,
            windows: summary,
            fetched_at: Utc::now(),
        };
    }

    let load = match post_json(
        http,
        api_base,
        &auth.access_token,
        "loadCodeAssist",
        &serde_json::json!({ "metadata": { "ideType": "ANTIGRAVITY" } }),
    )
    .await
    {
        Ok(v) => v,
        Err(ApiError::Unauthorized) => {
            return fail(
                auth.email.as_deref(),
                ProviderStatus::NeedsLogin,
                "Antigravity session rejected. Run `agy` to sign in again.",
            );
        }
        Err(ApiError::Other(msg)) => {
            return fail(auth.email.as_deref(), ProviderStatus::Error, &msg);
        }
    };

    let plan = load
        .pointer("/paidTier/name")
        .or_else(|| load.pointer("/currentTier/name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let project = load.get("cloudaicompanionProject").and_then(|v| v.as_str());

    let mut body = serde_json::Map::new();
    if let Some(p) = project {
        body.insert("project".into(), Value::String(p.to_string()));
    }

    let models = match post_json(
        http,
        api_base,
        &auth.access_token,
        "fetchAvailableModels",
        &Value::Object(body),
    )
    .await
    {
        Ok(v) => v,
        Err(ApiError::Unauthorized) => {
            return fail(
                auth.email.as_deref(),
                ProviderStatus::NeedsLogin,
                "Antigravity session rejected. Run `agy` to sign in again.",
            );
        }
        Err(ApiError::Other(msg)) => {
            return fail(auth.email.as_deref(), ProviderStatus::Error, &msg);
        }
    };

    let mut snap = parse_models(&models, auth.email.as_deref(), plan.as_deref());
    snap.fetched_at = Utc::now();
    snap
}

enum ApiError {
    Unauthorized,
    Other(String),
}

fn snapshot_id(email: Option<&str>) -> String {
    match email {
        Some(e) => format!("antigravity:{e}"),
        None => "antigravity".into(),
    }
}

/// Personal / consumer traffic is metered on the daily host. A non-Google
/// `api_base` (tests) stays on that base so the suite never calls Google.
fn summary_bases(api_base: &str, auth_method: Option<&str>) -> Vec<String> {
    let configured = api_base
        .trim_end_matches('/')
        .trim_end_matches(':')
        .to_string();
    if !configured.contains("googleapis.com") {
        return vec![configured];
    }
    let daily = "https://daily-cloudcode-pa.googleapis.com/v1internal".to_string();
    let prod = "https://cloudcode-pa.googleapis.com/v1internal".to_string();
    let business = auth_method.is_some_and(|method| {
        let method = method.to_ascii_lowercase();
        method.contains("business") || method.contains("enterprise")
    });
    if business {
        vec![prod, daily]
    } else {
        vec![daily, prod]
    }
}

fn hub_user_agent() -> String {
    let os = match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    format!("antigravity/hub/2.24.0 (aidev_client; os_type={os}; arch={arch}; cl=963137146)")
}

/// `retrieveUserQuotaSummary` on the hosts T3 probes. Empty when every host
/// is missing or returns no buckets, so the caller can fall back to models.
async fn fetch_summary_windows(
    http: &reqwest::Client,
    api_base: &str,
    auth: &AntigravityAuth,
) -> Vec<crate::models::UsageWindow> {
    let requested_at = Utc::now();
    let project = companion_project(http, api_base, &auth.access_token).await;
    let request_body = match project {
        Some(project) => serde_json::json!({ "project": project }),
        None => serde_json::json!({}),
    };
    let mut merged: Vec<crate::models::UsageWindow> = Vec::new();
    for base in summary_bases(api_base, auth.auth_method.as_deref()) {
        let Ok(body) = post_summary(http, &base, &auth.access_token, &request_body).await else {
            continue;
        };
        for window in parse_quota_summary(&body, requested_at) {
            if !merged.iter().any(|have| have.key == window.key) {
                merged.push(window);
            }
        }
    }
    merged
}

/// `retrieveUserQuotaSummary` answers 403 unless the Cloud Code project from
/// `loadCodeAssist` is in the body. Consumer projects are `aicode-consumers`.
async fn companion_project(
    http: &reqwest::Client,
    api_base: &str,
    token: &str,
) -> Option<String> {
    let base = if api_base.contains("googleapis.com") {
        "https://cloudcode-pa.googleapis.com/v1internal"
    } else {
        api_base
    };
    let body = post_json(
        http,
        base,
        token,
        "loadCodeAssist",
        &serde_json::json!({ "metadata": { "ideType": "ANTIGRAVITY" } }),
    )
    .await
    .ok()?;
    body.get("cloudaicompanionProject")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

async fn post_summary(
    http: &reqwest::Client,
    api_base: &str,
    token: &str,
    body: &Value,
) -> Result<Value, ()> {
    let url = endpoint(api_base, "retrieveUserQuotaSummary");
    let res = http
        .post(&url)
        .bearer_auth(token)
        .header(reqwest::header::USER_AGENT, hub_user_agent())
        .json(body)
        .send()
        .await
        .map_err(|_| ())?;
    if !res.status().is_success() {
        return Err(());
    }
    res.json::<Value>().await.map_err(|_| ())
}

fn endpoint(api_base: &str, method: &str) -> String {
    format!("{}:{}", api_base.trim_end_matches(':'), method)
}

async fn post_json(
    http: &reqwest::Client,
    api_base: &str,
    token: &str,
    method: &str,
    body: &Value,
) -> Result<Value, ApiError> {
    let url = endpoint(api_base, method);
    let res = http
        .post(&url)
        .bearer_auth(token)
        .header(reqwest::header::USER_AGENT, "antigravity")
        .json(body)
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    let status = res.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(ApiError::Unauthorized);
    }
    if !status.is_success() {
        return Err(ApiError::Other(format!("Antigravity API HTTP {status}")));
    }
    res.json::<Value>()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))
}

fn fail(email: Option<&str>, status: ProviderStatus, message: &str) -> ProviderSnapshot {
    let id = match email {
        Some(e) => format!("antigravity:{e}"),
        None => "antigravity".into(),
    };
    ProviderSnapshot {
        id,
        kind: "antigravity".into(),
        display_name: "Antigravity".into(),
        account: email.map(|s| s.to_string()),
        plan: None,
        status,
        message: Some(message.to_string()),
        windows: Vec::new(),
        fetched_at: Utc::now(),
    }
}
