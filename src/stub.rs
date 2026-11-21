use chrono::Utc;

use crate::catalog::ProviderDef;
use crate::models::{ProviderSnapshot, ProviderStatus};

pub fn stub_snapshot(entry: &ProviderDef) -> ProviderSnapshot {
    ProviderSnapshot {
        id: entry.kind.clone(),
        kind: entry.kind.clone(),
        display_name: entry.display_name.clone(),
        account: None,
        plan: None,
        status: ProviderStatus::NeedsLogin,
        message: Some("Stub: live fetch not wired yet".to_string()),
        windows: Vec::new(),
        fetched_at: Utc::now(),
    }
}
