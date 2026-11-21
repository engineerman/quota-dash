use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::config::Config;
use crate::providers::antigravity::PendingLogin;
use crate::refresh::QuotaCache;

#[derive(Debug, Clone)]
pub struct ProviderApis {
    pub cursor: String,
    pub codex: String,
    pub claude_api: String,
    pub claude_oauth: String,
    pub openrouter: String,
    pub opencode: String,
    pub antigravity: String,
}

impl Default for ProviderApis {
    fn default() -> Self {
        Self {
            cursor: "https://api2.cursor.sh".into(),
            codex: "https://chatgpt.com".into(),
            claude_api: "https://api.anthropic.com".into(),
            claude_oauth: "https://console.anthropic.com".into(),
            openrouter: "https://openrouter.ai".into(),
            opencode: "https://opencode.ai/zen/go/v1".into(),
            antigravity: "https://cloudcode-pa.googleapis.com/v1internal".into(),
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub config_path: PathBuf,
    pub config: Arc<Mutex<Config>>,
    pub http: reqwest::Client,
    pub apis: ProviderApis,
    pub cache: Arc<Mutex<QuotaCache>>,
    /// In-flight `agy acp --login` (at most one).
    pub antigravity_login: Arc<tokio::sync::Mutex<Option<PendingLogin>>>,
}

impl AppState {
    pub fn new(config_path: PathBuf, config: Config) -> Self {
        Self {
            config_path,
            config: Arc::new(Mutex::new(config)),
            http: reqwest::Client::new(),
            apis: ProviderApis::default(),
            cache: Arc::new(Mutex::new(QuotaCache::default())),
            antigravity_login: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    pub fn with_cursor_api_base(mut self, base: impl Into<String>) -> Self {
        self.apis.cursor = base.into();
        self
    }

    pub fn with_apis(mut self, apis: ProviderApis) -> Self {
        self.apis = apis;
        self
    }

    pub fn load(config_path: PathBuf) -> Self {
        let config = Config::load_or_default(&config_path);
        let state = Self::new(config_path, config);
        if let Ok(mut cache) = state.cache.lock() {
            crate::refresh::load_antigravity_last(&mut cache);
        }
        state
    }

    pub fn persist(&self) -> Result<(), String> {
        let cfg = self
            .config
            .lock()
            .map_err(|_| "config lock poisoned".to_string())?;
        cfg.save(&self.config_path)
            .map_err(|e| format!("save config: {e}"))
    }

    pub fn openrouter_key(&self) -> Option<String> {
        let cfg = self.config.lock().ok()?;
        let k = cfg.openrouter_api_key.trim();
        if k.is_empty() {
            None
        } else {
            Some(k.to_string())
        }
    }

    pub fn opencode_key(&self) -> Option<String> {
        let cfg = self.config.lock().ok()?;
        let k = cfg.opencode_api_key.trim();
        if k.is_empty() {
            None
        } else {
            Some(k.to_string())
        }
    }

    pub fn bump_cache(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.last_full = None;
        }
    }
}
