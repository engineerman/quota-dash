use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

pub const CRED_TARGET: &str = "gemini:antigravity";

#[derive(Debug, Clone)]
pub struct AntigravityAuth {
    pub access_token: String,
    pub email: Option<String>,
    pub refresh_token: Option<String>,
    pub expiry: Option<DateTime<Utc>>,
    /// `consumer` on the 1.5 CLI file. Business sign-ins use a different quota host.
    pub auth_method: Option<String>,
}

#[derive(Debug)]
pub enum AntigravityAuthError {
    NeedsLogin(String),
    Error(String),
}

/// Parse the WinCred / agy JSON blob (`id_token_email` + `token.*`).
pub fn parse_cred_blob(json: &str) -> Result<AntigravityAuth, AntigravityAuthError> {
    let v: Value = serde_json::from_str(json).map_err(|e| {
        AntigravityAuthError::NeedsLogin(format!(
            "Antigravity credential blob is not valid JSON. Run `agy` to sign in again. ({e})"
        ))
    })?;

    let email = v
        .get("id_token_email")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let tok = v.get("token").ok_or_else(|| {
        AntigravityAuthError::NeedsLogin(
            "Antigravity credential has no token. Run `agy` to sign in.".into(),
        )
    })?;

    let access = tok
        .get("access_token")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            AntigravityAuthError::NeedsLogin(
                "Antigravity access token missing. Run `agy` to sign in.".into(),
            )
        })?;

    let refresh = tok
        .get("refresh_token")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let expiry = tok
        .get("expiry")
        .and_then(|x| x.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc));

    let auth_method = v
        .get("auth_method")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    Ok(AntigravityAuth {
        access_token: access,
        email,
        refresh_token: refresh,
        expiry,
        auth_method,
    })
}

/// `%USERPROFILE%\.t3`, or `T3CODE_HOME` / `QUOTA_DASH_T3_HOME` when set.
pub fn t3_home() -> Option<PathBuf> {
    if let Some(over) = std::env::var_os("QUOTA_DASH_T3_HOME") {
        return Some(PathBuf::from(over));
    }
    if let Some(home) = std::env::var_os("T3CODE_HOME") {
        return Some(PathBuf::from(home));
    }
    let base = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(base).join(".t3"))
}

/// Read every `antigravity-oauth-token` under a T3 home.
///
/// The 1.5 CLI keeps one file per account and refreshes it itself. The file has
/// an access token and a refresh token, and no OAuth client, so we can use the
/// access token only while `expiry` is still ahead.
pub fn read_t3_oauth_auths(home: &Path) -> Vec<AntigravityAuth> {
    let root = home
        .join("userdata")
        .join("providers")
        .join("antigravity");
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry
            .path()
            .join("antigravity-acp")
            .join("antigravity-oauth-token");
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        if let Ok(auth) = parse_cred_blob(&text) {
            if auth.email.is_some() {
                out.push(auth);
            }
        }
    }
    out
}

/// When the stored access token is expired, take the same email's still-valid
/// token from T3's `antigravity-oauth-token` file.
pub fn revive_with_t3(stored: AntigravityAuth, home: &Path) -> AntigravityAuth {
    if access_token_fresh(&stored, Utc::now()) {
        return stored;
    }
    let Some(email) = stored.email.clone() else {
        return stored;
    };
    let Some(live) = freshest_t3(home, &email) else {
        return stored;
    };
    if !access_token_fresh(&live, Utc::now()) {
        return stored;
    }
    AntigravityAuth {
        access_token: live.access_token,
        email: stored.email.or(live.email),
        refresh_token: live.refresh_token.or(stored.refresh_token),
        expiry: live.expiry,
        auth_method: live.auth_method.or(stored.auth_method),
    }
}

pub fn revive_from_installed_t3(stored: AntigravityAuth) -> AntigravityAuth {
    match t3_home() {
        Some(home) => revive_with_t3(stored, &home),
        None => stored,
    }
}

/// Fresh T3 token for an email that has no vault blob.
pub fn fresh_t3_auth(email: &str) -> Option<AntigravityAuth> {
    let home = t3_home()?;
    let live = freshest_t3(&home, email)?;
    access_token_fresh(&live, Utc::now()).then_some(live)
}

fn freshest_t3(home: &Path, email: &str) -> Option<AntigravityAuth> {
    read_t3_oauth_auths(home)
        .into_iter()
        .filter(|auth| {
            auth.email
                .as_deref()
                .is_some_and(|e| e.eq_ignore_ascii_case(email))
        })
        .max_by_key(|auth| auth.expiry)
}

/// True when the access token is usable for about two more minutes.
pub fn access_token_fresh(auth: &AntigravityAuth, now: DateTime<Utc>) -> bool {
    match auth.expiry {
        Some(exp) => exp > now + chrono::Duration::minutes(2),
        None => !auth.access_token.is_empty(),
    }
}

/// Read active account from Windows Credential Manager target `gemini:antigravity`.
pub fn read_active_auth() -> Result<AntigravityAuth, AntigravityAuthError> {
    let blob = read_wincred_blob_raw(CRED_TARGET)?;
    parse_cred_blob(&blob)
}

/// Raw WinCred UTF-8 JSON blob (for vault sync).
pub fn read_wincred_blob_raw(target: &str) -> Result<String, AntigravityAuthError> {
    read_wincred_blob(target)
}

pub fn write_wincred_blob(
    target: &str,
    user_name: &str,
    blob: &[u8],
) -> Result<(), AntigravityAuthError> {
    write_wincred(target, user_name, blob)
}

pub fn delete_wincred(target: &str) -> Result<(), AntigravityAuthError> {
    delete_wincred_inner(target)
}

/// Ensure a fresh access token: use as-is if not expired; otherwise try OAuth refresh
/// when `ANTIGRAVITY_OAUTH_CLIENT_ID` / `ANTIGRAVITY_OAUTH_CLIENT_SECRET` are set.
pub async fn ensure_access_token(
    http: &reqwest::Client,
    auth: &AntigravityAuth,
) -> Result<AntigravityAuth, AntigravityAuthError> {
    if access_token_fresh(auth, Utc::now()) {
        return Ok(auth.clone());
    }

    let Some(refresh) = auth.refresh_token.as_deref() else {
        return Err(AntigravityAuthError::NeedsLogin(
            "Antigravity sign-in expired. Run `agy` to sign in again.".into(),
        ));
    };

    let client_id = std::env::var("ANTIGRAVITY_OAUTH_CLIENT_ID").ok();
    let client_secret = std::env::var("ANTIGRAVITY_OAUTH_CLIENT_SECRET").ok();
    let (Some(client_id), Some(client_secret)) = (client_id, client_secret) else {
        return Err(AntigravityAuthError::NeedsLogin(
            "Antigravity access token expired. Run `agy` to refresh sign-in.".into(),
        ));
    };

    refresh_access_token(
        http,
        refresh,
        &client_id,
        &client_secret,
        auth.email.clone(),
    )
    .await
}

pub async fn refresh_access_token(
    http: &reqwest::Client,
    refresh_token: &str,
    client_id: &str,
    client_secret: &str,
    email: Option<String>,
) -> Result<AntigravityAuth, AntigravityAuthError> {
    let res = http
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|e| AntigravityAuthError::Error(format!("OAuth refresh request failed: {e}")))?;

    let status = res.status();
    if !status.is_success() {
        return Err(AntigravityAuthError::NeedsLogin(format!(
            "Antigravity sign-in expired (OAuth refresh HTTP {status}). Run `agy` to sign in again."
        )));
    }

    let body: Value = res
        .json()
        .await
        .map_err(|e| AntigravityAuthError::Error(format!("OAuth refresh JSON: {e}")))?;

    let access = body
        .get("access_token")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            AntigravityAuthError::NeedsLogin(
                "OAuth refresh returned no access_token. Run `agy` to sign in again.".into(),
            )
        })?;

    let expires_in = body
        .get("expires_in")
        .and_then(|x| x.as_i64().or_else(|| x.as_u64().map(|u| u as i64)))
        .unwrap_or(3600);

    Ok(AntigravityAuth {
        access_token: access,
        email,
        refresh_token: Some(refresh_token.to_string()),
        expiry: Some(Utc::now() + chrono::Duration::seconds(expires_in)),
        auth_method: None,
    })
}

#[cfg(windows)]
fn read_wincred_blob(target: &str) -> Result<String, AntigravityAuthError> {
    use std::ptr;
    use windows::core::PCWSTR;
    use windows::Win32::Security::Credentials::{
        CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC,
    };

    let wide: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let mut cred_ptr: *mut CREDENTIALW = ptr::null_mut();
    let read = unsafe {
        CredReadW(
            PCWSTR(wide.as_ptr()),
            CRED_TYPE_GENERIC,
            None,
            &mut cred_ptr,
        )
    };
    if read.is_err() || cred_ptr.is_null() {
        return Err(AntigravityAuthError::NeedsLogin(
            "Sign in with `agy` (Antigravity CLI) once, then refresh.".into(),
        ));
    }

    let result = unsafe {
        let cred = &*cred_ptr;
        let size = cred.CredentialBlobSize as usize;
        if size == 0 || cred.CredentialBlob.is_null() {
            Err(AntigravityAuthError::NeedsLogin(
                "Antigravity credential is empty. Run `agy` to sign in.".into(),
            ))
        } else {
            let bytes = std::slice::from_raw_parts(cred.CredentialBlob, size);
            String::from_utf8(bytes.to_vec()).map_err(|e| {
                AntigravityAuthError::NeedsLogin(format!(
                    "Antigravity credential is not UTF-8. Run `agy` again. ({e})"
                ))
            })
        }
    };

    unsafe {
        CredFree(cred_ptr as *const _);
    }
    result
}

#[cfg(not(windows))]
fn read_wincred_blob(_target: &str) -> Result<String, AntigravityAuthError> {
    Err(AntigravityAuthError::NeedsLogin(
        "Antigravity WinCred discovery is only available on Windows. Run on Windows after `agy` sign-in."
            .into(),
    ))
}

#[cfg(windows)]
fn write_wincred(target: &str, user_name: &str, blob: &[u8]) -> Result<(), AntigravityAuthError> {
    use windows::core::PWSTR;
    use windows::Win32::Security::Credentials::{
        CredWriteW, CREDENTIALW, CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    let mut target_w: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let mut user_w: Vec<u16> = user_name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut blob_buf = blob.to_vec();
    let cred = CREDENTIALW {
        Flags: CRED_FLAGS(0),
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target_w.as_mut_ptr()),
        Comment: PWSTR::null(),
        LastWritten: Default::default(),
        CredentialBlobSize: blob_buf.len() as u32,
        CredentialBlob: blob_buf.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        AttributeCount: 0,
        Attributes: std::ptr::null_mut(),
        TargetAlias: PWSTR::null(),
        UserName: PWSTR(user_w.as_mut_ptr()),
    };
    match unsafe { CredWriteW(&cred, 0u32) } {
        Ok(()) => Ok(()),
        Err(_) => Err(AntigravityAuthError::Error(
            "WinCred CredWrite failed".into(),
        )),
    }
}

#[cfg(not(windows))]
fn write_wincred(
    _target: &str,
    _user_name: &str,
    _blob: &[u8],
) -> Result<(), AntigravityAuthError> {
    Err(AntigravityAuthError::Error(
        "WinCred write only on Windows".into(),
    ))
}

#[cfg(windows)]
fn delete_wincred_inner(target: &str) -> Result<(), AntigravityAuthError> {
    use windows::core::PCWSTR;
    use windows::Win32::Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC};

    let target_w: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let _ = unsafe { CredDeleteW(PCWSTR(target_w.as_ptr()), CRED_TYPE_GENERIC, None) };
    Ok(())
}

#[cfg(not(windows))]
fn delete_wincred_inner(_target: &str) -> Result<(), AntigravityAuthError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_cred_blob() {
        let blob = json!({
            "id_token_email": "you@gmail.com",
            "token": {
                "access_token": "ya29.access",
                "refresh_token": "1//refresh",
                "expiry": "2099-01-01T00:00:00Z"
            }
        })
        .to_string();
        let auth = parse_cred_blob(&blob).unwrap();
        assert_eq!(auth.access_token, "ya29.access");
        assert_eq!(auth.email.as_deref(), Some("you@gmail.com"));
        assert_eq!(auth.refresh_token.as_deref(), Some("1//refresh"));
        assert!(access_token_fresh(&auth, Utc::now()));
    }

    #[test]
    fn expired_token_not_fresh() {
        let auth = AntigravityAuth {
            access_token: "old".into(),
            email: None,
            refresh_token: Some("rt".into()),
            expiry: Some(Utc::now() - chrono::Duration::minutes(5)),
            auth_method: None,
        };
        assert!(!access_token_fresh(&auth, Utc::now()));
    }

    #[test]
    fn missing_token_is_needs_login() {
        let blob = json!({"id_token_email": "a@b.com"}).to_string();
        match parse_cred_blob(&blob) {
            Err(AntigravityAuthError::NeedsLogin(_)) => {}
            other => panic!("expected NeedsLogin, got {other:?}"),
        }
    }

    #[test]
    fn expired_vault_uses_fresh_t3_oauth_file() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir
            .path()
            .join("userdata")
            .join("providers")
            .join("antigravity")
            .join("abc")
            .join("antigravity-acp");
        std::fs::create_dir_all(&folder).unwrap();
        let body = json!({
            "auth_method": "consumer",
            "id_token_email": "you@gmail.com",
            "token": {
                "access_token": "ya29.fresh",
                "refresh_token": "1//r",
                "expiry": "2099-01-01T00:00:00Z"
            }
        });
        std::fs::write(folder.join("antigravity-oauth-token"), body.to_string()).unwrap();

        let stored = AntigravityAuth {
            access_token: "old".into(),
            email: Some("you@gmail.com".into()),
            refresh_token: Some("old-rt".into()),
            expiry: Some(Utc::now() - chrono::Duration::minutes(5)),
            auth_method: None,
        };
        let live = revive_with_t3(stored, dir.path());
        assert_eq!(live.access_token, "ya29.fresh");
        assert_eq!(live.auth_method.as_deref(), Some("consumer"));
        assert!(access_token_fresh(&live, Utc::now()));
    }

    #[test]
    fn fresh_vault_token_is_kept_when_t3_also_has_one() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir
            .path()
            .join("userdata")
            .join("providers")
            .join("antigravity")
            .join("abc")
            .join("antigravity-acp");
        std::fs::create_dir_all(&folder).unwrap();
        let body = json!({
            "id_token_email": "you@gmail.com",
            "token": {
                "access_token": "ya29.other",
                "expiry": "2099-01-01T00:00:00Z"
            }
        });
        std::fs::write(folder.join("antigravity-oauth-token"), body.to_string()).unwrap();

        let stored = AntigravityAuth {
            access_token: "vault-still-good".into(),
            email: Some("you@gmail.com".into()),
            refresh_token: None,
            expiry: Some(Utc::now() + chrono::Duration::hours(1)),
            auth_method: None,
        };
        let live = revive_with_t3(stored, dir.path());
        assert_eq!(live.access_token, "vault-still-good");
    }
}
