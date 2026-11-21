use std::net::SocketAddr;
use std::path::PathBuf;

use quota_dash::{app, AppState};

fn default_config_path() -> PathBuf {
    PathBuf::from("quota-dash.toml")
}

#[tokio::main]
async fn main() {
    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_config_path);

    let state = AppState::load(config_path.clone());
    let addr: SocketAddr = {
        let cfg = state.config.lock().expect("config lock");
        cfg.socket_addr()
            .parse()
            .unwrap_or_else(|_| "127.0.0.1:8787".parse().unwrap())
    };
    let refresh = state
        .config
        .lock()
        .expect("config lock")
        .refresh_interval_secs;

    println!(
        "quota-dash listening on http://{} — UI /  API /api/*  (config {}, refresh {}s)",
        addr,
        config_path.display(),
        refresh
    );

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    axum::serve(listener, app(state))
        .await
        .expect("server error");
}
