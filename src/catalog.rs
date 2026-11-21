use std::sync::OnceLock;

use serde::Deserialize;
use serde::Serialize;

use crate::i18n::{self, Lang, ProviderCopy};

/// How the UI should help the user finish setup.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SetupKind {
    App,
    Cli,
    OauthCode,
    ApiKey,
}

/// Provider registry row (`data/providers.json`).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ProviderDef {
    pub kind: String,
    pub display_name: String,
    pub setup: SetupKind,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub docs_url: Option<String>,
    #[serde(default)]
    pub dashboard_url: Option<String>,
    pub accepts_api_key: bool,
    #[serde(default)]
    pub secret_slot: Option<String>,
    pub logo: String,
}

#[derive(Debug, Deserialize)]
struct ProvidersFile {
    providers: Vec<ProviderDef>,
}

static REGISTRY: OnceLock<Vec<ProviderDef>> = OnceLock::new();

pub fn providers() -> &'static [ProviderDef] {
    REGISTRY.get_or_init(|| {
        let raw: ProvidersFile =
            serde_json::from_str(include_str!("../data/providers.json")).expect("providers.json");
        raw.providers
    })
}

/// Serialized catalog row for the API (localized strings).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CatalogEntryOut {
    pub kind: String,
    pub display_name: String,
    pub summary: String,
    pub setup: SetupKind,
    pub steps: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dashboard_url: Option<String>,
    pub accepts_api_key: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_slot: Option<String>,
    pub logo: String,
}

fn merge_copy(p: &ProviderDef, lang: Lang) -> (String, Vec<String>) {
    match i18n::provider_copy(lang, &p.kind) {
        Some(ProviderCopy { summary, steps }) => (summary.clone(), steps.clone()),
        None => (String::new(), Vec::new()),
    }
}

pub fn catalog_for_lang(lang: Lang) -> Vec<CatalogEntryOut> {
    providers()
        .iter()
        .map(|p| {
            let (summary, steps) = merge_copy(p, lang);
            CatalogEntryOut {
                kind: p.kind.clone(),
                display_name: p.display_name.clone(),
                summary,
                setup: p.setup,
                steps,
                command: p.command.clone(),
                docs_url: p.docs_url.clone(),
                dashboard_url: p.dashboard_url.clone(),
                accepts_api_key: p.accepts_api_key,
                secret_slot: p.secret_slot.clone(),
                logo: p.logo.clone(),
            }
        })
        .collect()
}

pub fn find(kind: &str) -> Option<&'static ProviderDef> {
    providers()
        .iter()
        .find(|e| e.kind.eq_ignore_ascii_case(kind))
}

pub fn display_name(kind: &str) -> Option<&str> {
    find(kind).map(|e| e.display_name.as_str())
}

pub fn canonical_kind(kind: &str) -> Option<&str> {
    find(kind).map(|e| e.kind.as_str())
}
