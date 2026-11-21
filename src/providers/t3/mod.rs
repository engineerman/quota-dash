//! Read T3 Code's provider status cache. Does not ask T3 to refresh.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::models::{ProviderSnapshot, ProviderStatus, UsageWindow};

pub fn fetch() -> Vec<ProviderSnapshot> {
    let Some(dir) = cache_dir() else {
        return vec![not_found()];
    };
    if !dir.is_dir() {
        return vec![not_found()];
    }
    read_cache(&dir)
}

/// `%USERPROFILE%\.t3\caches`, or `T3CODE_HOME/caches` when that directory exists.
pub fn cache_dir() -> Option<PathBuf> {
    if let Some(over) = std::env::var_os("QUOTA_DASH_T3_CACHE") {
        return Some(PathBuf::from(over));
    }
    if let Some(home) = std::env::var_os("T3CODE_HOME") {
        let caches = PathBuf::from(home).join("caches");
        if caches.is_dir() {
            return Some(caches);
        }
    }
    let base = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(base).join(".t3").join("caches"))
}

pub fn read_cache(dir: &Path) -> Vec<ProviderSnapshot> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return vec![not_found()],
    };
    let mut snaps = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("t3")
            .to_string();
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(snap) = snapshot_from_cache(dir, &stem, &value) {
            snaps.push(snap);
        }
    }
    snaps.sort_by(|a, b| {
        a.display_name
            .to_lowercase()
            .cmp(&b.display_name.to_lowercase())
            .then_with(|| a.account.cmp(&b.account))
            .then_with(|| a.id.cmp(&b.id))
    });
    if snaps.is_empty() {
        vec![none_reported()]
    } else {
        snaps
    }
}

fn snapshot_from_cache(cache_dir: &Path, id: &str, value: &Value) -> Option<ProviderSnapshot> {
    let limits = value.get("usageLimits")?;
    let raw_windows = limits.get("windows")?.as_array()?;
    let windows: Vec<UsageWindow> = raw_windows.iter().filter_map(parse_window).collect();
    if windows.is_empty() {
        return None;
    }
    let display_name = value
        .get("displayName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(id)
        .to_string();
    let account = account_name(cache_dir, id, value);
    let fetched_at = limits
        .get("checkedAt")
        .and_then(|v| v.as_str())
        .and_then(parse_time)
        .unwrap_or_else(Utc::now);
    Some(ProviderSnapshot {
        id: id.to_string(),
        kind: "t3".into(),
        display_name,
        account,
        plan: None,
        status: ProviderStatus::Ok,
        message: None,
        windows,
        fetched_at,
    })
}

/// Cursor puts the address in `auth.email`. Antigravity's status cache only
/// says "Google account"; the address is `id_token_email` on that instance's
/// oauth file (`userdata/providers/<driver>/<sha256(instanceId)>/...`).
fn account_name(cache_dir: &Path, id: &str, value: &Value) -> Option<String> {
    let auth = value.get("auth");
    if let Some(email) = text_field(auth, "email") {
        return Some(email);
    }
    if let Some(email) = oauth_email(cache_dir, id, value) {
        return Some(email);
    }
    let instance = instance_id(id, value);
    let driver = driver_name(instance, value);
    if let Some(label) = text_field(auth, "label") {
        if !generic_account_label(&label) {
            return Some(label);
        }
        if let Some(suffix) = instance_suffix(instance, &driver) {
            return Some(suffix);
        }
        return Some(label);
    }
    instance_suffix(instance, &driver)
}

fn text_field(value: Option<&Value>, key: &str) -> Option<String> {
    value
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn instance_id<'a>(id: &'a str, value: &'a Value) -> &'a str {
    value
        .get("instanceId")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(id)
}

fn driver_name(instance: &str, value: &Value) -> String {
    if let Some(driver) = value
        .get("driver")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return driver.to_string();
    }
    instance.split('_').next().unwrap_or(instance).to_string()
}

fn generic_account_label(label: &str) -> bool {
    matches!(
        label.trim().to_ascii_lowercase().as_str(),
        "google account" | "account"
    )
}

fn instance_suffix(instance: &str, driver: &str) -> Option<String> {
    let prefix = format!("{driver}_");
    instance
        .strip_prefix(&prefix)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn oauth_email(cache_dir: &Path, id: &str, value: &Value) -> Option<String> {
    if cache_dir.file_name().and_then(|s| s.to_str()) != Some("caches") {
        return None;
    }
    let home = cache_dir.parent()?;
    let instance = instance_id(id, value);
    let driver = driver_name(instance, value);
    let path = home
        .join("userdata")
        .join("providers")
        .join(&driver)
        .join(sha256_hex(instance.as_bytes()))
        .join(format!("{driver}-acp"))
        .join(format!("{driver}-oauth-token"));
    let text = std::fs::read_to_string(path).ok()?;
    let parsed: Value = serde_json::from_str(&text).ok()?;
    text_field(Some(&parsed), "id_token_email")
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let dig = Sha256::digest(bytes);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(dig.len() * 2);
    for byte in dig {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

fn parse_window(value: &Value) -> Option<UsageWindow> {
    let label = value
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let key = value
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(label.as_str())
        .to_string();
    if key.is_empty() {
        return None;
    }
    let used_percent = value
        .get("usedPercent")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let label = if label.is_empty() { key.clone() } else { label };
    Some(UsageWindow {
        key,
        label,
        used_percent,
        resets_at: value
            .get("resetsAt")
            .and_then(|v| v.as_str())
            .and_then(parse_time),
        detail: None,
    })
}

fn parse_time(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn not_found() -> ProviderSnapshot {
    ProviderSnapshot {
        id: "t3".into(),
        kind: "t3".into(),
        display_name: "T3 Code".into(),
        account: None,
        plan: None,
        status: ProviderStatus::NeedsLogin,
        message: Some("T3 Code cache was not found on this PC.".into()),
        windows: Vec::new(),
        fetched_at: Utc::now(),
    }
}

fn none_reported() -> ProviderSnapshot {
    ProviderSnapshot {
        id: "t3".into(),
        kind: "t3".into(),
        display_name: "T3 Code".into(),
        account: None,
        plan: None,
        status: ProviderStatus::Ok,
        message: Some("T3 Code reported no quota windows.".into()),
        windows: Vec::new(),
        fetched_at: Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn keeps_accounts_that_reported_windows() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "cursor.json",
            r#"{
                "displayName": "Cursor",
                "auth": {"email": "ada@example.com", "label": "Cursor account"},
                "usageLimits": {
                    "checkedAt": "2026-11-21T16:32:48Z",
                    "windows": [
                        {"id": "autoPercentUsed", "label": "Cursor Models", "usedPercent": 0.08, "resetsAt": "2026-12-07T17:55:46Z"},
                        {"id": "apiPercentUsed", "label": "Other Models", "usedPercent": 3.78, "resetsAt": "2026-12-07T17:55:46Z"}
                    ]
                }
            }"#,
        );
        write(
            dir.path(),
            "claudeAgent.json",
            r#"{"displayName":"Claude","usageLimits":{"windows":[],"unavailable":{"reason":"unsupported"}}}"#,
        );
        write(
            dir.path(),
            "antigravity.json",
            r#"{
                "displayName": "Antigravity",
                "auth": {"label": "Google account"},
                "usageLimits": {
                    "checkedAt": "2026-11-21T16:32:14Z",
                    "windows": [{"id": "gemini-quota", "label": "Gemini · Reported quota", "usedPercent": 0}]
                }
            }"#,
        );

        let snaps = read_cache(dir.path());
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].display_name, "Antigravity");
        assert_eq!(snaps[0].account.as_deref(), Some("Google account"));
        assert_eq!(snaps[0].windows.len(), 1);
        assert_eq!(snaps[1].display_name, "Cursor");
        assert_eq!(snaps[1].account.as_deref(), Some("ada@example.com"));
        assert_eq!(snaps[1].windows.len(), 2);
        assert!((snaps[1].windows[0].used_percent - 0.08).abs() < 0.001);
        assert!(snaps[1].windows[0].resets_at.is_some());
        assert!(snaps[0].fetched_at < snaps[1].fetched_at);
    }

    fn write_oauth(home: &Path, instance: &str, driver: &str, email: &str) {
        let path = home
            .join("userdata")
            .join("providers")
            .join(driver)
            .join(sha256_hex(instance.as_bytes()))
            .join(format!("{driver}-acp"))
            .join(format!("{driver}-oauth-token"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            format!(r#"{{"auth_method":"consumer","id_token_email":"{email}"}}"#),
        )
        .unwrap();
    }

    #[test]
    fn antigravity_rows_use_the_google_address() {
        let root = tempfile::tempdir().unwrap();
        let caches = root.path().join("caches");
        std::fs::create_dir_all(&caches).unwrap();
        let body = |instance: &str| {
            format!(
                r#"{{
                    "displayName": "Antigravity",
                    "instanceId": "{instance}",
                    "driver": "antigravity",
                    "auth": {{"label": "Google account", "status": "authenticated"}},
                    "usageLimits": {{
                        "checkedAt": "2026-11-21T16:37:14Z",
                        "windows": [{{"id": "gemini-quota", "label": "Gemini", "usedPercent": 0}}]
                    }}
                }}"#
            )
        };
        write(&caches, "antigravity.json", &body("antigravity"));
        write(
            &caches,
            "antigravity_eelight.json",
            &body("antigravity_eelight"),
        );
        write_oauth(root.path(), "antigravity", "antigravity", "admin@labkit.app");
        write_oauth(
            root.path(),
            "antigravity_eelight",
            "antigravity",
            "eelightworks@gmail.com",
        );

        let snaps = read_cache(&caches);
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].id, "antigravity");
        assert_eq!(snaps[0].account.as_deref(), Some("admin@labkit.app"));
        assert_eq!(snaps[1].id, "antigravity_eelight");
        assert_eq!(snaps[1].account.as_deref(), Some("eelightworks@gmail.com"));
    }

    #[test]
    fn missing_directory_asks_for_t3() {
        let snap = &read_cache(Path::new("this-t3-cache-does-not-exist"))[0];
        assert_eq!(snap.status, ProviderStatus::NeedsLogin);
        assert!(snap.windows.is_empty());
    }

    #[test]
    fn empty_cache_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "note.txt", "skip");
        let snap = &read_cache(dir.path())[0];
        assert_eq!(snap.status, ProviderStatus::Ok);
        assert!(snap.message.as_deref().unwrap().contains("no quota"));
    }
}
