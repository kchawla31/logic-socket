//! OAuth 2 token commands for the Auth editor.

use serde::Serialize;
use tauri::State;

use crate::{AppState, CmdResult, e};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenStatus {
    has_token: bool,
    /// First/last characters only — the token itself never reaches the UI.
    preview: Option<String>,
    expires_at: Option<i64>,
    has_refresh_token: bool,
    scope: Option<String>,
    token_type: Option<String>,
}

fn status(state: &AppState, owner_id: &str) -> CmdResult<TokenStatus> {
    let t = state.engine.oauth2_cached(owner_id).map_err(e)?;
    Ok(match t {
        Some(t) => TokenStatus {
            has_token: !t.access_token.is_empty(),
            preview: Some(if t.access_token.len() > 12 {
                format!(
                    "{}…{}",
                    &t.access_token[..6],
                    &t.access_token[t.access_token.len() - 4..]
                )
            } else {
                "••••".into()
            }),
            expires_at: t.expires_at,
            has_refresh_token: t.refresh_token.is_some(),
            scope: t.scope.clone(),
            token_type: t.token_type.clone(),
        },
        None => TokenStatus {
            has_token: false,
            preview: None,
            expires_at: None,
            has_refresh_token: false,
            scope: None,
            token_type: None,
        },
    })
}

#[tauri::command]
pub fn oauth2_status(state: State<'_, AppState>, owner_id: String) -> CmdResult<TokenStatus> {
    status(&state, &owner_id)
}

/// Get a token now (browser sign-in for the authorization code grant).
#[tauri::command]
pub async fn oauth2_authorize(
    state: State<'_, AppState>,
    owner_id: String,
) -> CmdResult<TokenStatus> {
    let cfg = state.engine.oauth2_config(&owner_id).map_err(e)?;
    let opener = |url: &str| open_in_browser(url);
    state
        .engine
        .oauth2_authorize(&owner_id, &cfg, &opener)
        .await
        .map_err(e)?;
    status(&state, &owner_id)
}

/// MCP sign-in: discover the server's authorization server, register if needed, sign in.
#[tauri::command]
pub async fn mcp_oauth_sign_in(
    state: State<'_, AppState>,
    server_id: String,
) -> CmdResult<TokenStatus> {
    let opener = |url: &str| open_in_browser(url);
    state
        .engine
        .mcp_oauth_sign_in(&server_id, &opener)
        .await
        .map_err(e)?;
    status(&state, &server_id)
}

#[tauri::command]
pub fn oauth2_clear(state: State<'_, AppState>, owner_id: String) -> CmdResult<()> {
    state.engine.oauth2_clear(&owner_id).map_err(e)
}

/// Open a URL in the user's default browser.
pub fn open_in_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}
