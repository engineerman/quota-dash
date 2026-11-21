//! Multi-account vault for Antigravity quota *display*.
//!
//! Tracked index = which accounts appear in Kota. WinCred is only a credential
//! source for login / refresh — listing never auto-readds untracked emails.

use serde::{Deserialize, Serialize};

use super::auth::{
    delete_wincred, parse_cred_blob, read_wincred_blob_raw, write_wincred_blob, AntigravityAuth,
    AntigravityAuthError, CRED_TARGET,
};
use crate::secret_store;

const INDEX_KEY: &str = "antigravity:index";

fn vault_key(email: &str) -> String {
    format!("antigravity:account:{}", email.to_ascii_lowercase())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    pub email: String,
    /// True when this email matches the current WinCred blob (informational only).
    pub active: bool,
}

fn load_index() -> Vec<String> {
    secret_store::get(INDEX_KEY)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_index(idx: &[String]) -> Result<(), String> {
    let body = serde_json::to_string(idx).map_err(|e| e.to_string())?;
    secret_store::set(INDEX_KEY, Some(&body))
}

fn is_tracked(email: &str) -> bool {
    load_index().iter().any(|e| e.eq_ignore_ascii_case(email))
}

/// Active WinCred email, if any.
pub fn active_email() -> Option<String> {
    let blob = read_wincred_blob_raw(CRED_TARGET).ok()?;
    parse_cred_blob(&blob).ok()?.email
}

/// Update vault blob for WinCred account **only if already tracked**. Never grows the index.
pub fn refresh_active_blob() -> Option<String> {
    let blob = read_wincred_blob_raw(CRED_TARGET).ok()?;
    let auth = parse_cred_blob(&blob).ok()?;
    let email = auth.email?;
    if !is_tracked(&email) {
        return None;
    }
    let _ = secret_store::set(&vault_key(&email), Some(&blob));
    Some(email)
}

/// Explicitly track an account (after successful login / import).
pub fn track(email: &str, blob: Option<&str>) -> Result<(), String> {
    let email = email.trim();
    if email.is_empty() {
        return Err("email empty".into());
    }
    if let Some(b) = blob {
        secret_store::set(&vault_key(email), Some(b))?;
    } else if let Ok(raw) = read_wincred_blob_raw(CRED_TARGET) {
        if let Ok(auth) = parse_cred_blob(&raw) {
            if auth
                .email
                .as_deref()
                .is_some_and(|e| e.eq_ignore_ascii_case(email))
            {
                secret_store::set(&vault_key(email), Some(&raw))?;
            }
        }
    }
    let mut idx = load_index();
    if !idx.iter().any(|e| e.eq_ignore_ascii_case(email)) {
        idx.push(email.to_string());
        save_index(&idx)?;
    }
    Ok(())
}

/// After login: persist WinCred into vault + tracked index.
pub fn track_active() -> Option<String> {
    let blob = read_wincred_blob_raw(CRED_TARGET).ok()?;
    let auth = parse_cred_blob(&blob).ok()?;
    let email = auth.email?;
    let _ = track(&email, Some(&blob));
    Some(email)
}

/// Tracked accounts for quota UI (does **not** auto-import WinCred).
pub fn list() -> Vec<Account> {
    let _ = refresh_active_blob();
    let wincred = active_email();
    load_index()
        .into_iter()
        .map(|email| Account {
            active: wincred
                .as_ref()
                .is_some_and(|a| a.eq_ignore_ascii_case(&email)),
            email,
        })
        .collect()
}

pub fn vault_json(email: &str) -> Option<String> {
    secret_store::get(&vault_key(email))
}

pub fn save_vault_json(email: &str, json: &str) -> Result<(), String> {
    // Token refresh only — never grow the tracked index.
    if !is_tracked(email) {
        return Ok(());
    }
    secret_store::set(&vault_key(email), Some(json))
}

/// Make a vaulted account the active WinCred session (API kept; UI does not expose).
pub fn switch(email: &str) -> Result<(), String> {
    let _ = refresh_active_blob();
    let match_email = load_index()
        .into_iter()
        .find(|e| e.eq_ignore_ascii_case(email))
        .ok_or_else(|| format!("Bilinmeyen Antigravity hesabı: {email}"))?;
    let json = vault_json(&match_email)
        .ok_or_else(|| "Kayıtlı token yok; tekrar giriş yap.".to_string())?;
    write_wincred_blob(CRED_TARGET, "antigravity", json.as_bytes())
        .map_err(|e| format!("{e:?}"))?;
    Ok(())
}

/// Stop tracking one account (quota list). Does not change agy WinCred.
pub fn remove(email: &str) -> Result<(), String> {
    secret_store::set(&vault_key(email), None)?;
    let idx: Vec<String> = load_index()
        .into_iter()
        .filter(|e| !e.eq_ignore_ascii_case(email))
        .collect();
    save_index(&idx)
}

/// Before a new Google login: refresh tracked blob if current WinCred is tracked, then clear WinCred.
pub fn prepare_add_account() -> Option<String> {
    let prev = refresh_active_blob().or_else(active_email);
    let _ = delete_wincred(CRED_TARGET);
    prev
}

/// Auth for a tracked email (WinCred if matches, else vault blob).
pub fn auth_for(email: &str) -> Result<AntigravityAuth, AntigravityAuthError> {
    if let Some(active) = active_email() {
        if active.eq_ignore_ascii_case(email) {
            return parse_cred_blob(&read_wincred_blob_raw(CRED_TARGET)?);
        }
    }
    let json = vault_json(email).ok_or_else(|| {
        AntigravityAuthError::NeedsLogin(format!("No vaulted Antigravity session for {email}."))
    })?;
    parse_cred_blob(&json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret_store::set_secrets_dir_for_test;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn untrack_stays_untracked_without_wincred_reseed() {
        let _guard = crate::secret_store::TEST_SECRETS_LOCK.lock().unwrap();
        let dir = tempdir().unwrap();
        set_secrets_dir_for_test(dir.path());
        let blob = json!({
            "id_token_email": "a@x.com",
            "token": { "access_token": "t", "refresh_token": "r", "expiry": "2099-01-01T00:00:00Z" }
        })
        .to_string();
        track("a@x.com", Some(&blob)).unwrap();
        assert_eq!(list().len(), 1);
        remove("a@x.com").unwrap();
        assert!(list().is_empty());
        // refresh_active_blob cannot resurrect without WinCred + track
        assert!(refresh_active_blob().is_none());
        assert!(list().is_empty());
    }

    #[test]
    fn vault_index_roundtrip() {
        let _guard = crate::secret_store::TEST_SECRETS_LOCK.lock().unwrap();
        let dir = tempdir().unwrap();
        set_secrets_dir_for_test(dir.path());
        let blob = json!({
            "id_token_email": "a@x.com",
            "token": { "access_token": "t", "refresh_token": "r", "expiry": "2099-01-01T00:00:00Z" }
        })
        .to_string();
        track("a@x.com", Some(&blob)).unwrap();
        assert_eq!(load_index(), vec!["a@x.com".to_string()]);
        assert!(vault_json("a@x.com").is_some());
        let refreshed = json!({
            "id_token_email": "a@x.com",
            "token": { "access_token": "t2", "refresh_token": "r", "expiry": "2099-01-01T00:00:00Z" }
        })
        .to_string();
        save_vault_json("a@x.com", &refreshed).unwrap();
        assert!(vault_json("a@x.com").unwrap().contains("t2"));
        remove("a@x.com").unwrap();
        assert!(load_index().is_empty());
        assert!(vault_json("a@x.com").is_none());
        // Untracked refresh must not resurrect index membership.
        save_vault_json("a@x.com", &blob).unwrap();
        assert!(load_index().is_empty());
    }
}
