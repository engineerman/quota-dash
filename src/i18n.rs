//! Locales and copy loaded from `data/locales/*.json` (embedded at build time).

use std::collections::HashMap;
use std::sync::OnceLock;

use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

/// A shipped locale. Unknown tags resolve to English.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Lang {
    code: &'static str,
}

impl Default for Lang {
    fn default() -> Self {
        Self::en()
    }
}

impl Lang {
    pub fn en() -> Self {
        Self { code: "en" }
    }

    pub fn parse(s: &str) -> Self {
        match_tag(s).unwrap_or_else(Self::en)
    }

    pub fn as_tag(self) -> &'static str {
        self.code
    }

    /// First shipped language in an `Accept-Language` list, otherwise English.
    pub fn from_accept_language(header: &str) -> Self {
        for part in header.split(',') {
            let tag = part.split(';').next().unwrap_or("").trim();
            if let Some(lang) = match_tag(tag) {
                return lang;
            }
        }
        Self::en()
    }

    pub fn from_headers(headers: &HeaderMap) -> Self {
        headers
            .get(axum::http::header::ACCEPT_LANGUAGE)
            .and_then(|v| v.to_str().ok())
            .map(Self::from_accept_language)
            .unwrap_or_else(Self::en)
    }

    pub fn resolve(headers: &HeaderMap, query: Option<&str>) -> Self {
        query
            .map(Self::parse)
            .unwrap_or_else(|| Self::from_headers(headers))
    }
}

#[derive(Debug, Deserialize)]
struct LocaleFile {
    lang: String,
    fallback: String,
    ui: HashMap<String, String>,
    providers: HashMap<String, ProviderCopy>,
    errors: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderCopy {
    pub summary: String,
    pub steps: Vec<String>,
}

const LOCALE_JSON: &[(&str, &str)] = &[
    ("ar", include_str!("../data/locales/ar.json")),
    ("de", include_str!("../data/locales/de.json")),
    ("en", include_str!("../data/locales/en.json")),
    ("es", include_str!("../data/locales/es.json")),
    ("fr", include_str!("../data/locales/fr.json")),
    ("hi", include_str!("../data/locales/hi.json")),
    ("id", include_str!("../data/locales/id.json")),
    ("it", include_str!("../data/locales/it.json")),
    ("ja", include_str!("../data/locales/ja.json")),
    ("ko", include_str!("../data/locales/ko.json")),
    ("nl", include_str!("../data/locales/nl.json")),
    ("pl", include_str!("../data/locales/pl.json")),
    ("pt", include_str!("../data/locales/pt.json")),
    ("ru", include_str!("../data/locales/ru.json")),
    ("tr", include_str!("../data/locales/tr.json")),
    ("uk", include_str!("../data/locales/uk.json")),
    ("vi", include_str!("../data/locales/vi.json")),
    ("zh", include_str!("../data/locales/zh.json")),
];

static LOCALES: OnceLock<HashMap<&'static str, LocaleFile>> = OnceLock::new();

fn parse_locale(json: &str) -> LocaleFile {
    serde_json::from_str(json).expect("locale JSON must parse")
}

fn locales() -> &'static HashMap<&'static str, LocaleFile> {
    LOCALES.get_or_init(|| {
        let mut map = HashMap::new();
        for (code, json) in LOCALE_JSON {
            let key = static_code(code).unwrap_or_else(|| {
                panic!("locale {code} is embedded but missing from languages.json")
            });
            let file = parse_locale(json);
            if file.lang != *code {
                panic!("locale {code} has lang field {}", file.lang);
            }
            map.insert(key, file);
        }
        for entry in available_languages() {
            if !map.contains_key(entry.code.as_str()) {
                panic!(
                    "languages.json lists {} but no locale file was embedded",
                    entry.code
                );
            }
        }
        map
    })
}

fn locale(lang: Lang) -> &'static LocaleFile {
    locales()
        .get(lang.as_tag())
        .or_else(|| locales().get("en"))
        .expect("en locale")
}

pub fn fallback_for(lang: Lang) -> Lang {
    Lang::parse(&locale(lang).fallback)
}

pub fn provider_copy(lang: Lang, kind: &str) -> Option<&'static ProviderCopy> {
    let primary = locale(lang);
    if let Some(p) = primary.providers.get(kind) {
        return Some(p);
    }
    let fall = locale(fallback_for(lang));
    fall.providers.get(kind)
}

fn lookup_error(lang: Lang, key: &str) -> Option<String> {
    locale(lang)
        .errors
        .get(key)
        .cloned()
        .or_else(|| locale(fallback_for(lang)).errors.get(key).cloned())
}

/// Localized server/user message from `errors` map.
pub fn error(lang: Lang, key: &str) -> String {
    lookup_error(lang, key).unwrap_or_else(|| key.to_string())
}

/// Replace `{name}` placeholders in an error template.
pub fn format(lang: Lang, key: &str, vars: &[(&str, &str)]) -> String {
    let mut s = error(lang, key);
    for (k, v) in vars {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

pub fn ui(lang: Lang, key: &str) -> String {
    let primary = locale(lang);
    primary
        .ui
        .get(key)
        .cloned()
        .or_else(|| locale(fallback_for(lang)).ui.get(key).cloned())
        .unwrap_or_else(|| key.to_string())
}

/// UI string map for `GET /api/locales/:lang`.
pub fn ui_bundle(lang: Lang) -> &'static HashMap<String, String> {
    &locale(lang).ui
}

/// `ltr` or `rtl` for the language picker entry.
pub fn text_dir(lang: Lang) -> &'static str {
    available_languages()
        .iter()
        .find(|e| e.code.as_str() == lang.as_tag())
        .filter(|e| e.dir == "rtl")
        .map(|_| "rtl")
        .unwrap_or("ltr")
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LanguageEntry {
    pub code: String,
    pub label: String,
    #[serde(default = "default_dir")]
    pub dir: String,
}

fn default_dir() -> String {
    "ltr".into()
}

#[derive(Debug, Deserialize)]
struct LanguagesFile {
    languages: Vec<LanguageEntry>,
}

static LANGUAGES: OnceLock<Vec<LanguageEntry>> = OnceLock::new();

pub fn available_languages() -> &'static [LanguageEntry] {
    LANGUAGES.get_or_init(|| {
        let raw: LanguagesFile =
            serde_json::from_str(include_str!("../data/languages.json")).expect("languages.json");
        raw.languages
    })
}

pub fn is_known_locale(code: &str) -> bool {
    match_tag(code).is_some()
}

/// Operating-system UI language when it matches a shipped locale, otherwise English.
///
/// Windows: user preferred UI languages, in order. Linux and other Unix:
/// `LC_ALL`, then `LC_MESSAGES`, then `LANG`.
pub fn system_lang() -> Lang {
    for tag in os_locale_tags() {
        if let Some(lang) = match_tag(&tag) {
            return lang;
        }
    }
    Lang::en()
}

fn static_code(code: &str) -> Option<&'static str> {
    let code = code.trim();
    if code.is_empty() {
        return None;
    }
    available_languages().iter().find_map(|e| {
        if e.code.eq_ignore_ascii_case(code) {
            Some(e.code.as_str())
        } else {
            None
        }
    })
}

/// Map `en-US`, `zh_CN`, `pt-BR` onto a shipped primary tag.
fn match_tag(raw: &str) -> Option<Lang> {
    let normalized = raw.trim().to_ascii_lowercase().replace('_', "-");
    let normalized = normalized
        .split(['.', '@'])
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"');
    if normalized.is_empty() || normalized == "c" || normalized == "posix" {
        return None;
    }
    if let Some(code) = static_code(normalized) {
        return Some(Lang { code });
    }
    let primary = normalized.split('-').next().unwrap_or("");
    static_code(primary).map(|code| Lang { code })
}

#[cfg(windows)]
fn os_locale_tags() -> Vec<String> {
    use windows::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};

    let mut count = 0u32;
    let mut chars = 0u32;
    let sized = unsafe {
        GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, None, &mut chars).is_ok()
    };
    if sized && chars >= 2 {
        let mut buf = vec![0u16; chars as usize];
        let ok = unsafe {
            GetUserPreferredUILanguages(
                MUI_LANGUAGE_NAME,
                &mut count,
                Some(windows::core::PWSTR(buf.as_mut_ptr())),
                &mut chars,
            )
            .is_ok()
        };
        if ok {
            let names = String::from_utf16_lossy(&buf);
            let tags: Vec<String> = names
                .split('\0')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect();
            if !tags.is_empty() {
                return tags;
            }
        }
    }
    ui_language_tag().into_iter().collect()
}

#[cfg(windows)]
fn ui_language_tag() -> Option<String> {
    use windows::Win32::Globalization::{GetUserDefaultUILanguage, LCIDToLocaleName};

    let langid = unsafe { GetUserDefaultUILanguage() };
    let mut buf = [0u16; 85];
    let n = unsafe { LCIDToLocaleName(u32::from(langid), Some(&mut buf), 0) };
    if n <= 1 {
        return None;
    }
    let end = (n as usize).saturating_sub(1).min(buf.len());
    let tag = String::from_utf16_lossy(&buf[..end]);
    if tag.is_empty() {
        None
    } else {
        Some(tag)
    }
}

#[cfg(not(windows))]
fn os_locale_tags() -> Vec<String> {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        let Ok(value) = std::env::var(key) else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() || value == "C" || value == "POSIX" || value.starts_with("C.") {
            continue;
        }
        return vec![value.to_string()];
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_fold_to_shipped_primary_language() {
        assert_eq!(Lang::parse("de-DE").as_tag(), "de");
        assert_eq!(Lang::parse("zh_CN").as_tag(), "zh");
        assert_eq!(Lang::parse("pt-BR").as_tag(), "pt");
        assert_eq!(Lang::parse("sv-SE").as_tag(), "en");
        assert_eq!(
            Lang::from_accept_language("sv-SE,sv;q=0.9,en;q=0.8").as_tag(),
            "en"
        );
        assert_eq!(
            Lang::from_accept_language("fr-CA,fr;q=0.9,en;q=0.8").as_tag(),
            "fr"
        );
    }

    #[test]
    fn every_locale_covers_english_keys() {
        let en = locale(Lang::en());
        assert_eq!(available_languages().len(), LOCALE_JSON.len());
        for entry in available_languages() {
            let lang = Lang::parse(&entry.code);
            assert_eq!(lang.as_tag(), entry.code);
            let file = locale(lang);
            for key in en.ui.keys() {
                assert!(file.ui.contains_key(key), "{} missing ui.{key}", entry.code);
            }
            for key in en.providers.keys() {
                let copy = file
                    .providers
                    .get(key)
                    .unwrap_or_else(|| panic!("{} missing providers.{key}", entry.code));
                assert!(
                    !copy.summary.is_empty(),
                    "{} empty summary {key}",
                    entry.code
                );
                assert!(!copy.steps.is_empty(), "{} empty steps {key}", entry.code);
            }
            for key in en.errors.keys() {
                let text = file
                    .errors
                    .get(key)
                    .unwrap_or_else(|| panic!("{} missing errors.{key}", entry.code));
                if en.errors[key].contains("{detail}") {
                    assert!(text.contains("{detail}"), "{} {key}", entry.code);
                }
                if en.errors[key].contains("{code}") {
                    assert!(text.contains("{code}"), "{} {key}", entry.code);
                }
            }
            assert_eq!(file.fallback, "en");
        }
        assert_eq!(text_dir(Lang::parse("ar")), "rtl");
        assert_eq!(text_dir(Lang::parse("tr")), "ltr");
    }
}
