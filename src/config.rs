use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    pub bind: String,
    pub port: u16,
    /// Refresh interval in seconds.
    pub refresh_interval_secs: u64,
    /// Per-provider fetch timeout in seconds.
    #[serde(default = "default_provider_timeout_secs")]
    pub provider_timeout_secs: u64,
    /// How long a last-good ok snapshot may be reused after a failure.
    #[serde(default = "default_cache_ttl_secs")]
    pub cache_ttl_secs: u64,
    #[serde(default)]
    pub enabled_providers: Vec<String>,
    /// Optional OpenRouter API key (else `OPENROUTER_API_KEY` env).
    #[serde(default)]
    pub openrouter_api_key: String,
    /// Optional OpenCode Go API key when local discovery finds none.
    #[serde(default)]
    pub opencode_api_key: String,
}

fn default_provider_timeout_secs() -> u64 {
    20
}
fn default_cache_ttl_secs() -> u64 {
    1800
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1".to_string(),
            port: 8787,
            refresh_interval_secs: 60,
            provider_timeout_secs: default_provider_timeout_secs(),
            cache_ttl_secs: default_cache_ttl_secs(),
            enabled_providers: Vec::new(),
            openrouter_api_key: String::new(),
            opencode_api_key: String::new(),
        }
    }
}

impl Config {
    pub fn from_str(toml_src: &str) -> Result<Self, toml::de::Error> {
        let mut cfg: Config = toml::from_str(toml_src)?;
        if cfg.bind.is_empty() {
            cfg.bind = Self::default().bind;
        }
        if cfg.port == 0 {
            cfg.port = Self::default().port;
        }
        if cfg.refresh_interval_secs == 0 {
            cfg.refresh_interval_secs = Self::default().refresh_interval_secs;
        }
        if cfg.provider_timeout_secs == 0 {
            cfg.provider_timeout_secs = Self::default().provider_timeout_secs;
        }
        if cfg.cache_ttl_secs == 0 {
            cfg.cache_ttl_secs = Self::default().cache_ttl_secs;
        }
        Ok(cfg)
    }

    pub fn load_or_default(path: &std::path::Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(s) => Self::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let body = toml::to_string_pretty(self)?;
        std::fs::write(path, body)?;
        Ok(())
    }

    pub fn socket_addr(&self) -> String {
        format!("{}:{}", self.bind, self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_binds_localhost() {
        let c = Config::default();
        assert_eq!(c.bind, "127.0.0.1");
        assert!(c.port > 0);
        assert!(c.refresh_interval_secs > 0);
    }

    #[test]
    fn parses_toml_overrides() {
        let c = Config::from_str(
            r#"
bind = "127.0.0.1"
port = 9999
refresh_interval_secs = 30
enabled_providers = ["cursor"]
openrouter_api_key = "sk-or-test"
"#,
        )
        .unwrap();
        assert_eq!(c.port, 9999);
        assert_eq!(c.refresh_interval_secs, 30);
        assert_eq!(c.enabled_providers, vec!["cursor".to_string()]);
        assert_eq!(c.openrouter_api_key, "sk-or-test");
    }

    #[test]
    fn openrouter_api_key_defaults_empty() {
        let c = Config::from_str(
            r#"
bind = "127.0.0.1"
port = 8787
refresh_interval_secs = 60
"#,
        )
        .unwrap();
        assert_eq!(c.openrouter_api_key, "");
    }
}
