use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct CodexAuth {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub account_id: Option<String>,
}

#[derive(Debug)]
pub enum CodexAuthError {
    NeedsLogin(String),
    Error(String),
}

/// `CODEX_HOME/auth.json`, else `%USERPROFILE%\.codex\auth.json` (or `$HOME`).
pub fn default_auth_path() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(home).join("auth.json"));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|v| !v.is_empty())?;
    Some(PathBuf::from(home).join(".codex").join("auth.json"))
}

/// Reads tokens from a Codex `auth.json` written by `codex login`.
pub fn read_auth(path: &Path) -> Result<CodexAuth, CodexAuthError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| CodexAuthError::Error(format!("Could not read Codex auth.json: {e}")))?;
    parse_auth_json(&text)
}

pub fn parse_auth_json(text: &str) -> Result<CodexAuth, CodexAuthError> {
    let root: Value = serde_json::from_str(text)
        .map_err(|e| CodexAuthError::Error(format!("Codex auth.json is not valid JSON: {e}")))?;
    let tokens = root.get("tokens").ok_or_else(|| {
        CodexAuthError::NeedsLogin(
            "Codex is using an API key, not a ChatGPT login. Run `codex login`.".into(),
        )
    })?;

    let access = tokens
        .get("access_token")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let Some(access_token) = access else {
        return Err(CodexAuthError::NeedsLogin(
            "Codex is using an API key, not a ChatGPT login. Run `codex login`.".into(),
        ));
    };

    let refresh_token = tokens
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let account_id = tokens
        .get("account_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    Ok(CodexAuth {
        access_token,
        refresh_token,
        account_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tokens_object() {
        let auth = parse_auth_json(
            r#"{
              "tokens": {
                "access_token": "at",
                "refresh_token": "rt",
                "account_id": "acc-1"
              }
            }"#,
        )
        .unwrap();
        assert_eq!(auth.access_token, "at");
        assert_eq!(auth.refresh_token.as_deref(), Some("rt"));
        assert_eq!(auth.account_id.as_deref(), Some("acc-1"));
    }

    #[test]
    fn missing_access_token_is_needs_login() {
        match parse_auth_json(r#"{"tokens":{"api_key":"sk-x"}}"#) {
            Err(CodexAuthError::NeedsLogin(_)) => {}
            other => panic!("expected NeedsLogin, got {other:?}"),
        }
    }

    #[test]
    fn reads_auth_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(
            &path,
            r#"{"tokens":{"access_token":"tok","refresh_token":"ref"}}"#,
        )
        .unwrap();
        let auth = read_auth(&path).unwrap();
        assert_eq!(auth.access_token, "tok");
    }
}
