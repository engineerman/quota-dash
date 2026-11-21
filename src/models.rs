use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    Ok,
    NeedsLogin,
    /// Last successful reading kept on screen after the access token expired.
    Stale,
    Error,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageWindow {
    pub key: String,
    pub label: String,
    pub used_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderSnapshot {
    pub id: String,
    pub kind: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    pub status: ProviderStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub windows: Vec<UsageWindow>,
    pub fetched_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageReport {
    pub generated_at: DateTime<Utc>,
    pub providers: Vec<ProviderSnapshot>,
}

impl UsageReport {
    pub fn empty() -> Self {
        Self {
            generated_at: Utc::now(),
            providers: Vec::new(),
        }
    }
}
