use rusqlite::{Connection, OpenFlags};

use crate::chromium_safe_storage::{self, CURSOR_PREFIX};

#[derive(Debug, Clone)]
pub struct CursorAuth {
    pub token: String,
    pub email: Option<String>,
    pub membership: Option<String>,
}

#[derive(Debug)]
pub enum CursorAuthError {
    NeedsLogin(String),
    Error(String),
}

/// Reads Cursor auth keys from `state.vscdb` and unseals the access token when needed.
pub fn read_auth_from_db(state_db: &std::path::Path) -> Result<CursorAuth, CursorAuthError> {
    let sealed = read_item(state_db, "cursorAuth/accessToken")
        .map_err(|e| CursorAuthError::Error(format!("Could not read Cursor login: {e}")))?;
    let email = read_item(state_db, "cursorAuth/cachedEmail").ok().flatten();
    let membership = read_item(state_db, "cursorAuth/stripeMembershipType")
        .ok()
        .flatten();

    let Some(sealed) = sealed.filter(|s| !s.is_empty()) else {
        return Err(CursorAuthError::NeedsLogin(
            "Sign in to the Cursor app on this PC, then refresh.".into(),
        ));
    };

    let key = if sealed.starts_with(CURSOR_PREFIX) {
        chromium_safe_storage::read_local_state_key("Cursor")
    } else {
        None
    };

    let token = chromium_safe_storage::unseal(&sealed, key.as_deref()).map_err(|e| {
        CursorAuthError::NeedsLogin(format!(
            "Cursor keeps its sign-in sealed and it could not be opened. Sign out and in again in Cursor, then refresh. ({e})"
        ))
    })?;

    Ok(CursorAuth {
        token,
        email,
        membership,
    })
}

fn read_item(state_db: &std::path::Path, key: &str) -> Result<Option<String>, String> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(state_db, flags).map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA mmap_size=0; PRAGMA cache_size=-2000;")
        .map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT value FROM ItemTable WHERE key=?1")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt.query([key]).map_err(|e| e.to_string())?;
    match rows.next().map_err(|e| e.to_string())? {
        Some(row) => Ok(row.get::<_, Option<String>>(0).map_err(|e| e.to_string())?),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn reads_plaintext_token_from_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.vscdb");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT);
                 INSERT INTO ItemTable VALUES ('cursorAuth/accessToken', 'test-token');
                 INSERT INTO ItemTable VALUES ('cursorAuth/cachedEmail', 'a@b.com');
                 INSERT INTO ItemTable VALUES ('cursorAuth/stripeMembershipType', 'pro');",
            )
            .unwrap();
        }
        let auth = read_auth_from_db(&db).unwrap();
        assert_eq!(auth.token, "test-token");
        assert_eq!(auth.email.as_deref(), Some("a@b.com"));
        assert_eq!(auth.membership.as_deref(), Some("pro"));
    }

    #[test]
    fn missing_token_is_needs_login() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.vscdb");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch("CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT);")
                .unwrap();
        }
        match read_auth_from_db(&db) {
            Err(CursorAuthError::NeedsLogin(_)) => {}
            other => panic!("expected NeedsLogin, got {other:?}"),
        }
    }
}
