mod auth;
mod parse;

pub use auth::{read_auth_from_db, CursorAuth, CursorAuthError};
pub use parse::parse_usage;

use crate::models::{ProviderSnapshot, ProviderStatus};
use chrono::Utc;
use serde_json::Value;

const DEFAULT_STATE_DB: &str = "Cursor/User/globalStorage/state.vscdb";

pub fn default_state_db_path() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(|a| std::path::PathBuf::from(a).join(DEFAULT_STATE_DB))
}

pub async fn fetch(http: &reqwest::Client, api_base: &str) -> ProviderSnapshot {
    let Some(db) = default_state_db_path() else {
        return fail(ProviderStatus::NeedsLogin, "APPDATA not set.");
    };
    if !db.exists() {
        return fail(
            ProviderStatus::NeedsLogin,
            "Sign in to the Cursor app on this PC, then refresh.",
        );
    }

    let auth = match read_auth_from_db(&db) {
        Ok(a) => a,
        Err(CursorAuthError::NeedsLogin(msg)) => return fail(ProviderStatus::NeedsLogin, &msg),
        Err(CursorAuthError::Error(msg)) => return fail(ProviderStatus::Error, &msg),
    };

    fetch_with_auth(http, api_base, &auth).await
}

pub async fn fetch_with_auth(
    http: &reqwest::Client,
    api_base: &str,
    auth: &CursorAuth,
) -> ProviderSnapshot {
    let usage = match post_json(
        http,
        api_base,
        auth.token.as_str(),
        "aiserver.v1.DashboardService/GetCurrentPeriodUsage",
    )
    .await
    {
        Ok(v) => v,
        Err(ApiError::Unauthorized) => {
            return fail(
                ProviderStatus::NeedsLogin,
                "Cursor session rejected. Sign in again in the Cursor app.",
            );
        }
        Err(ApiError::Other(msg)) => return fail(ProviderStatus::Error, &msg),
    };

    let plan_name = match post_json(
        http,
        api_base,
        auth.token.as_str(),
        "aiserver.v1.DashboardService/GetPlanInfo",
    )
    .await
    {
        Ok(v) => v
            .pointer("/planInfo/planName")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        Err(_) => None,
    };

    let mut snap = parse_usage(
        &usage,
        auth.email.as_deref(),
        auth.membership.as_deref(),
        plan_name.as_deref(),
    );
    snap.fetched_at = Utc::now();
    snap
}

enum ApiError {
    Unauthorized,
    Other(String),
}

async fn post_json(
    http: &reqwest::Client,
    api_base: &str,
    token: &str,
    path: &str,
) -> Result<Value, ApiError> {
    let url = format!(
        "{}/{}",
        api_base.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    let res = http
        .post(&url)
        .bearer_auth(token)
        .header("Connect-Protocol-Version", "1")
        .header("x-cursor-client-type", "cli")
        .json(&serde_json::json!({}))
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    let status = res.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(ApiError::Unauthorized);
    }
    if !status.is_success() {
        return Err(ApiError::Other(format!("Cursor API HTTP {status}")));
    }
    res.json::<Value>()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))
}

fn fail(status: ProviderStatus, message: &str) -> ProviderSnapshot {
    ProviderSnapshot {
        id: "cursor".into(),
        kind: "cursor".into(),
        display_name: "Cursor".into(),
        account: None,
        plan: None,
        status,
        message: Some(message.to_string()),
        windows: Vec::new(),
        fetched_at: Utc::now(),
    }
}
