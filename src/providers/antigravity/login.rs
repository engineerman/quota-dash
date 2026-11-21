//! In-UI Antigravity login via `agy acp --login` (URL + authorization code on stdin).

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;
use tokio::time::timeout;

use super::auth::{read_active_auth, AntigravityAuth};
use crate::i18n::Lang;

/// Default wait for `agy` to print the Google auth URL.
const URL_WAIT: Duration = Duration::from_secs(20);
/// After submitting a code, wait for `agy` to finish writing credentials.
const CODE_WAIT: Duration = Duration::from_secs(90);

pub struct PendingLogin {
    child: Child,
    stdin: Option<ChildStdin>,
    pub auth_url: String,
    pub lang: Lang,
    /// Captured stderr/stdout tails for error messages.
    log: Arc<Mutex<String>>,
    /// Keeps drain tasks alive until login finishes.
    _drains: Vec<tokio::task::JoinHandle<()>>,
}

impl std::fmt::Debug for PendingLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingLogin")
            .field("auth_url", &self.auth_url)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum LoginError {
    AgyMissing,
    Spawn(String),
    NoUrl(String),
    Code(String),
    Cancelled,
}

impl LoginError {
    pub fn message(&self, lang: Lang) -> String {
        match self {
            LoginError::AgyMissing => crate::i18n::error(lang, "antigravity.agy_missing"),
            LoginError::Cancelled => crate::i18n::error(lang, "antigravity.sign_in_cancelled"),
            LoginError::Spawn(s) | LoginError::NoUrl(s) | LoginError::Code(s) => s.clone(),
        }
    }
}

/// Resolve `agy.exe` on PATH, then `%LOCALAPPDATA%\agy\bin\agy.exe`.
pub fn find_agy() -> Option<PathBuf> {
    if let Ok(p) = which_agy() {
        return Some(p);
    }
    let local = std::env::var_os("LOCALAPPDATA")?;
    let candidate = PathBuf::from(local).join("agy").join("bin").join("agy.exe");
    if candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

/// Executable that accepts a URL arg and exits without opening a browser.
fn browser_noop() -> &'static str {
    #[cfg(windows)]
    {
        // `where` ignores the URL and exits 0 — never ShellExecute's a browser.
        "where.exe"
    }
    #[cfg(not(windows))]
    {
        "true"
    }
}

fn which_agy() -> Result<PathBuf, ()> {
    let path = std::env::var_os("PATH").ok_or(())?;
    for dir in std::env::split_paths(&path) {
        let exe = dir.join("agy.exe");
        if exe.is_file() {
            return Ok(exe);
        }
        let unix = dir.join("agy");
        if unix.is_file() {
            return Ok(unix);
        }
    }
    Err(())
}

/// Start `agy acp --login`, capture the Google OAuth URL, keep stdin open for the code.
///
/// Important: stdout/stderr stay drained in background tasks. Dropping the pipes after
/// reading the URL deadlocks `agy` (pipe buffer full) so it never reads the pasted code.
pub async fn start(lang: Lang) -> Result<PendingLogin, LoginError> {
    let agy = find_agy().ok_or(LoginError::AgyMissing)?;

    let mut cmd = Command::new(&agy);
    cmd.arg("acp")
        .arg("--login")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // UI opens the auth URL once. Prevent agy / webbrowser crates from
        // also launching a second tab (BROWSER=echo is ignored on some Windows builds).
        .env("BROWSER", browser_noop())
        .env("NO_BROWSER", "1")
        .kill_on_drop(true);

    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd.spawn().map_err(|e| {
        LoginError::Spawn(crate::i18n::format(
            lang,
            "antigravity.spawn_failed",
            &[("detail", &e.to_string())],
        ))
    })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| LoginError::Spawn(crate::i18n::error(lang, "antigravity.no_stdout")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| LoginError::Spawn(crate::i18n::error(lang, "antigravity.no_stderr")))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| LoginError::Spawn(crate::i18n::error(lang, "antigravity.no_stdin")))?;

    let log = Arc::new(Mutex::new(String::new()));
    let (url_tx, url_rx) = oneshot::channel::<String>();
    let url_tx = Arc::new(Mutex::new(Some(url_tx)));

    let log_out = Arc::clone(&log);
    let url_slot = Arc::clone(&url_tx);
    let out_drain = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            append_log(&log_out, "out", &line);
            if let Some(url) = extract_google_auth_url(&line) {
                if let Ok(mut g) = url_slot.lock() {
                    if let Some(tx) = g.take() {
                        let _ = tx.send(url);
                    }
                }
            }
        }
    });

    let log_err = Arc::clone(&log);
    let err_drain = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            append_log(&log_err, "err", &line);
        }
    });

    let auth_url = match timeout(URL_WAIT, url_rx).await {
        Ok(Ok(url)) => url,
        Ok(Err(_)) => {
            let _ = child.kill().await;
            let head = crate::i18n::error(lang, "antigravity.closed_without_url");
            return Err(LoginError::NoUrl(format!(
                "{head}\n{}",
                trunc(&snapshot_log(&log), 500)
            )));
        }
        Err(_) => {
            let _ = child.kill().await;
            let head = crate::i18n::error(lang, "antigravity.url_timeout");
            return Err(LoginError::NoUrl(format!(
                "{head}\n{}",
                trunc(&snapshot_log(&log), 500)
            )));
        }
    };

    Ok(PendingLogin {
        child,
        stdin: Some(stdin),
        auth_url,
        lang,
        log,
        _drains: vec![out_drain, err_drain],
    })
}

fn append_log(log: &Arc<Mutex<String>>, tag: &str, line: &str) {
    if let Ok(mut g) = log.lock() {
        g.push_str(tag);
        g.push_str(": ");
        g.push_str(line);
        g.push('\n');
        // Keep tail only.
        if g.len() > 4000 {
            let start = g.len() - 3000;
            g.replace_range(0..start, "");
        }
    }
}

fn snapshot_log(log: &Arc<Mutex<String>>) -> String {
    log.lock().map(|g| g.clone()).unwrap_or_default()
}

fn extract_google_auth_url(text: &str) -> Option<String> {
    for token in text.split_whitespace() {
        let t = token.trim_matches(|c: char| c == '"' || c == '\'' || c == '`' || c == ',');
        if t.starts_with("https://accounts.google.com/") {
            return Some(t.to_string());
        }
    }
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("https://accounts.google.com/") {
            return Some(t.to_string());
        }
    }
    None
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

/// Feed the browser authorization code (or full callback URL) into the waiting `agy`.
pub async fn submit_code(
    pending: &mut PendingLogin,
    code: &str,
) -> Result<AntigravityAuth, LoginError> {
    let lang = pending.lang;
    let code = normalize_code(code);
    if code.is_empty() {
        return Err(LoginError::Code(crate::i18n::error(
            lang,
            "antigravity.code_empty",
        )));
    }

    let mut stdin = pending
        .stdin
        .take()
        .ok_or_else(|| LoginError::Code(crate::i18n::error(lang, "antigravity.session_closed")))?;

    stdin.write_all(code.as_bytes()).await.map_err(|e| {
        LoginError::Code(crate::i18n::format(
            lang,
            "antigravity.code_write_failed",
            &[("detail", &e.to_string())],
        ))
    })?;
    stdin.write_all(b"\n").await.map_err(|e| {
        LoginError::Code(crate::i18n::format(
            lang,
            "antigravity.code_write_failed",
            &[("detail", &e.to_string())],
        ))
    })?;
    stdin.flush().await.map_err(|e| {
        LoginError::Code(crate::i18n::format(
            lang,
            "antigravity.flush_failed",
            &[("detail", &e.to_string())],
        ))
    })?;
    // EOF so interactive readers that wait for end-of-input can finish.
    drop(stdin);

    let status = match timeout(CODE_WAIT, pending.child.wait()).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            let head = crate::i18n::format(
                lang,
                "antigravity.wait_error",
                &[("detail", &e.to_string())],
            );
            return Err(LoginError::Code(format!(
                "{head}\n{}",
                trunc(&snapshot_log(&pending.log), 400)
            )));
        }
        Err(_) => {
            let _ = pending.child.kill().await;
            let head = crate::i18n::error(lang, "antigravity.code_timeout");
            return Err(LoginError::Code(format!(
                "{head}\n{}",
                trunc(&snapshot_log(&pending.log), 400)
            )));
        }
    };

    if !status.success() {
        let code = status.code().unwrap_or(-1).to_string();
        let head = crate::i18n::format(lang, "antigravity.exit_failed", &[("code", &code)]);
        return Err(LoginError::Code(format!(
            "{head}\n{}",
            trunc(&snapshot_log(&pending.log), 400)
        )));
    }

    // agy writes WinCred; give the OS a beat, then read.
    for _ in 0..15 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if let Ok(auth) = read_active_auth() {
            return Ok(auth);
        }
    }

    let head = crate::i18n::error(lang, "antigravity.no_wincred");
    Err(LoginError::Code(format!(
        "{head}\n{}",
        trunc(&snapshot_log(&pending.log), 400)
    )))
}

/// Accept raw code or full `https://antigravity.google/oauth-callback?...` URL.
fn normalize_code(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    if let Some(rest) = raw
        .strip_prefix("http://")
        .or_else(|| raw.strip_prefix("https://"))
    {
        // Full callback URL — pass through; agy accepts it.
        if rest.contains("oauth-callback") || rest.contains("code=") {
            return raw.to_string();
        }
    }
    // Strip accidental wrapping quotes/spaces.
    raw.trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace())
        .to_string()
}

pub async fn cancel(pending: &mut PendingLogin) {
    pending.stdin.take();
    let _ = pending.child.kill().await;
    let _ = pending.child.wait().await;
}

#[cfg(test)]
mod tests {
    use super::{extract_google_auth_url, normalize_code};

    #[test]
    fn extracts_url_from_agy_output_line() {
        let line = "  https://accounts.google.com/o/oauth2/auth?client_id=x&state=y";
        let url = extract_google_auth_url(line).unwrap();
        assert!(url.starts_with("https://accounts.google.com/"));
    }

    #[test]
    fn normalize_keeps_callback_url() {
        let u = "https://antigravity.google/oauth-callback?code=abc&state=x";
        assert_eq!(normalize_code(u), u);
    }

    #[test]
    fn normalize_trims_raw_code() {
        assert_eq!(normalize_code("  4/0AUn6Hd-jex  "), "4/0AUn6Hd-jex");
    }
}
