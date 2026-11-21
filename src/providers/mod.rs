pub mod antigravity;
pub mod claude_code;
pub mod codex;
pub mod cursor;
pub mod opencode;
pub mod openrouter;
pub mod t3;

use crate::models::ProviderSnapshot;
use crate::state::AppState;
use crate::stub;

/// Fetch live (or stub) snapshot(s) for one enabled provider kind.
pub async fn fetch_kind(kind: &str, state: &AppState) -> Vec<ProviderSnapshot> {
    match kind {
        "cursor" => vec![cursor::fetch(&state.http, &state.apis.cursor).await],
        "codex" => vec![codex::fetch(&state.http, &state.apis.codex).await],
        "claude-code" => vec![
            claude_code::fetch(
                &state.http,
                &state.apis.claude_api,
                &state.apis.claude_oauth,
            )
            .await,
        ],
        "antigravity" => antigravity::fetch_all(&state.http, &state.apis.antigravity).await,
        "opencode" => vec![
            opencode::fetch(
                &state.http,
                &state.apis.opencode,
                state.opencode_key().as_deref(),
            )
            .await,
        ],
        "openrouter" => vec![
            openrouter::fetch(
                &state.http,
                &state.apis.openrouter,
                state.openrouter_key().as_deref(),
            )
            .await,
        ],
        "t3" => t3::fetch(),
        other => crate::catalog::find(other)
            .map(|e| vec![stub::stub_snapshot(e)])
            .unwrap_or_default(),
    }
}
