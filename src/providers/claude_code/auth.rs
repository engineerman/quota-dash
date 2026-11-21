use std::path::{Path, PathBuf};

use chrono::{Duration, Utc};
use serde_json::{json, Value};

const OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

#[derive(Debug, Clone)]
pub struct ClaudeOAuth {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Unix epoch milliseconds (Claude Code credentials format).
    pub expires_at_ms: i64,
    pub rate_limit_tier: Option<String>,
    pub subscription_type: Option<String>,
}

#[derive(Debug)]
pub enum CredError {
    NeedsLogin(String),
    Error(String),
}

/// Resolves `CLAUDE_CONFIG_DIR` or `~/.claude`, then `.credentials.json`.
pub fn credentials_path() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir).join(".credentials.json"));
        }
    }
    dirs_home().map(|h| h.join(".claude").join(".credentials.json"))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

pub fn read_credentials(path: &Path) -> Result<ClaudeOAuth, CredError> {
    if !path.exists() {
        return Err(CredError::NeedsLogin(
            "Run `claude` and /login, then refresh.".into(),
        ));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| CredError::Error(format!("Could not read Claude Code credentials: {e}")))?;
    parse_credentials_json(&text)
}

pub fn parse_credentials_json(text: &str) -> Result<ClaudeOAuth, CredError> {
    let root: Value = serde_json::from_str(text)
        .map_err(|e| CredError::Error(format!("Invalid Claude Code credentials JSON: {e}")))?;
    let o = root
        .get("claudeAiOauth")
        .ok_or_else(|| CredError::NeedsLogin("No Claude Code OAuth in credentials file.".into()))?;

    let access = o
        .get("accessToken")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let refresh = o
        .get("refreshToken")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let expires_at_ms = o
        .get("expiresAt")
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_u64().map(|u| u as i64))
                .or_else(|| v.as_f64().map(|f| f as i64))
        })
        .unwrap_or(0);

    if access.is_empty() && refresh.is_none() {
        return Err(CredError::NeedsLogin(
            "Claude Code credentials have no access or refresh token. Run `claude` and /login."
                .into(),
        ));
    }

    Ok(ClaudeOAuth {
        access_token: access,
        refresh_token: refresh,
        expires_at_ms,
        rate_limit_tier: o
            .get("rateLimitTier")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        subscription_type: o
            .get("subscriptionType")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    })
}

impl ClaudeOAuth {
    pub fn is_expired_soon(&self) -> bool {
        let threshold = Utc::now() + Duration::minutes(1);
        let expires = chrono::DateTime::<Utc>::from_timestamp_millis(self.expires_at_ms)
            .unwrap_or(chrono::DateTime::<Utc>::UNIX_EPOCH);
        self.access_token.is_empty() || expires < threshold
    }
}

/// Refresh access token via Anthropic console OAuth; optionally rewrite credentials file.
pub async fn refresh_oauth(
    http: &reqwest::Client,
    oauth_base: &str,
    oauth: &ClaudeOAuth,
    credentials_path: Option<&Path>,
) -> Result<ClaudeOAuth, CredError> {
    let refresh = oauth.refresh_token.as_deref().filter(|s| !s.is_empty()).ok_or_else(|| {
        CredError::NeedsLogin(
            "Claude Code access token expired and no refresh token is available. Run `claude` and /login."
                .into(),
        )
    })?;

    let url = format!("{}/v1/oauth/token", oauth_base.trim_end_matches('/'));
    let body = json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh,
        "client_id": OAUTH_CLIENT_ID,
    });

    let res = http
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| CredError::Error(format!("OAuth refresh request failed: {e}")))?;
    let status = res.status();
    if !status.is_success() {
        return Err(CredError::NeedsLogin(format!(
            "Claude Code OAuth refresh failed (HTTP {status}). Run `claude` and /login."
        )));
    }
    let resp: Value = res
        .json()
        .await
        .map_err(|e| CredError::Error(format!("OAuth refresh JSON: {e}")))?;

    let access = resp
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CredError::Error("OAuth refresh missing access_token.".into()))?
        .to_string();
    let new_refresh = resp
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| oauth.refresh_token.clone());
    let expires_in = resp
        .get("expires_in")
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_u64().map(|u| u as i64))
                .or_else(|| v.as_f64().map(|f| f as i64))
        })
        .unwrap_or(3600);
    let expires_at_ms = (Utc::now() + Duration::seconds(expires_in)).timestamp_millis();

    let updated = ClaudeOAuth {
        access_token: access,
        refresh_token: new_refresh,
        expires_at_ms,
        rate_limit_tier: oauth.rate_limit_tier.clone(),
        subscription_type: oauth.subscription_type.clone(),
    };

    if let Some(path) = credentials_path {
        if let Err(e) = write_refreshed_credentials(path, &updated) {
            // Prefer returning the fresh in-memory token even if disk write fails.
            let _ = e;
        }
    }

    Ok(updated)
}

fn write_refreshed_credentials(path: &Path, oauth: &ClaudeOAuth) -> Result<(), String> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|_| "{}".into());
    let mut root: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    let obj = root
        .as_object_mut()
        .ok_or_else(|| "credentials root not an object".to_string())?;
    let entry = obj
        .entry("claudeAiOauth".to_string())
        .or_insert_with(|| json!({}));
    let map = entry
        .as_object_mut()
        .ok_or_else(|| "claudeAiOauth not an object".to_string())?;
    map.insert("accessToken".into(), json!(oauth.access_token));
    if let Some(r) = &oauth.refresh_token {
        map.insert("refreshToken".into(), json!(r));
    }
    map.insert("expiresAt".into(), json!(oauth.expires_at_ms));

    let out = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, out.as_bytes()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_oauth_block() {
        let text = r#"{
          "claudeAiOauth": {
            "accessToken": "at",
            "refreshToken": "rt",
            "expiresAt": 9999999999999,
            "rateLimitTier": "default_claude_max_5x",
            "subscriptionType": "max"
          }
        }"#;
        let o = parse_credentials_json(text).unwrap();
        assert_eq!(o.access_token, "at");
        assert_eq!(o.refresh_token.as_deref(), Some("rt"));
        assert_eq!(o.rate_limit_tier.as_deref(), Some("default_claude_max_5x"));
        assert!(!o.is_expired_soon());
    }

    #[test]
    fn missing_oauth_is_needs_login() {
        match parse_credentials_json("{}") {
            Err(CredError::NeedsLogin(_)) => {}
            other => panic!("expected NeedsLogin, got {other:?}"),
        }
    }
}
