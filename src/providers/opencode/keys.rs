use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct OpenCodeKey {
    pub id: String,
    pub label: String,
    pub key: String,
    pub active: bool,
}

/// `%USERPROFILE%\.local\share\opencode` (or `$HOME` on Unix).
pub fn default_opencode_dir() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|v| !v.is_empty())?;
    Some(
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("opencode"),
    )
}

/// Discovers OpenCode Go API keys from local SQLite + legacy `auth.json`.
pub fn discover_keys(opencode_dir: &Path) -> Vec<OpenCodeKey> {
    let mut list = Vec::new();
    read_keys_from_db(&opencode_dir.join("opencode.db"), &mut list);
    read_keys_from_auth_json(&opencode_dir.join("auth.json"), &mut list);
    list
}

fn read_keys_from_db(db: &Path, list: &mut Vec<OpenCodeKey>) {
    if !db.exists() {
        return;
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = match Connection::open_with_flags(db, flags) {
        Ok(c) => c,
        Err(_) => return,
    };
    let _ = conn.execute_batch("PRAGMA mmap_size=0; PRAGMA cache_size=-2000;");
    let mut stmt = match conn.prepare(
        "SELECT id, label, value, active FROM credential WHERE integration_id = 'opencode-go'",
    ) {
        Ok(s) => s,
        Err(_) => return,
    };
    let rows = match stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let label: Option<String> = row.get(1)?;
        let value: String = row.get(2)?;
        let active: Option<i64> = row.get(3)?;
        Ok((id, label, value, active))
    }) {
        Ok(r) => r,
        Err(_) => return,
    };
    for row in rows.flatten() {
        let (id, label, value, active) = row;
        if let Some(key) = parse_key_from_value_json(&value) {
            list.push(OpenCodeKey {
                id,
                label: label
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| "OpenCode".into()),
                key,
                active: active == Some(1),
            });
        }
    }
}

fn read_keys_from_auth_json(path: &Path, list: &mut Vec<OpenCodeKey>) {
    if !path.exists() {
        return;
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return,
    };
    let root: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return,
    };
    let Some(key) = root
        .pointer("/opencode-go/key")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
    else {
        return;
    };
    if list.iter().any(|k| k.key == key) {
        return;
    }
    let active = list.is_empty();
    list.push(OpenCodeKey {
        id: "auth-json".into(),
        label: "OpenCode".into(),
        key,
        active,
    });
}

fn parse_key_from_value_json(value: &str) -> Option<String> {
    let v: Value = serde_json::from_str(value).ok()?;
    v.get("key")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn discovers_key_from_sqlite_credential() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE credential (
                    id TEXT PRIMARY KEY,
                    label TEXT,
                    value TEXT,
                    active INTEGER,
                    integration_id TEXT
                 );
                 INSERT INTO credential VALUES (
                    'k1', 'Go Main', '{\"key\":\"sk-from-db\"}', 1, 'opencode-go'
                 );
                 INSERT INTO credential VALUES (
                    'other', 'X', '{\"key\":\"nope\"}', 1, 'other-integration'
                 );",
            )
            .unwrap();
        }
        let keys = discover_keys(dir.path());
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, "sk-from-db");
        assert_eq!(keys[0].label, "Go Main");
        assert!(keys[0].active);
    }

    #[test]
    fn discovers_legacy_auth_json_key() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.json"),
            r#"{"opencode-go":{"key":"sk-legacy"}}"#,
        )
        .unwrap();
        let keys = discover_keys(dir.path());
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].id, "auth-json");
        assert_eq!(keys[0].key, "sk-legacy");
        assert!(keys[0].active);
    }

    #[test]
    fn auth_json_skips_duplicate_of_db_key() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE credential (
                    id TEXT PRIMARY KEY, label TEXT, value TEXT, active INTEGER, integration_id TEXT
                 );
                 INSERT INTO credential VALUES ('k1', 'A', '{\"key\":\"sk-same\"}', 1, 'opencode-go');",
            )
            .unwrap();
        }
        std::fs::write(
            dir.path().join("auth.json"),
            r#"{"opencode-go":{"key":"sk-same"}}"#,
        )
        .unwrap();
        let keys = discover_keys(dir.path());
        assert_eq!(keys.len(), 1);
    }

    #[test]
    fn empty_dir_yields_no_keys() {
        let dir = tempfile::tempdir().unwrap();
        assert!(discover_keys(dir.path()).is_empty());
    }
}
