# quota-dash data

Product copy and UI strings live here — not in Rust. Add a provider by editing **`providers.json`** (technical card metadata) and every locale file under **`locales/`** (`providers.<kind>.summary` + `steps`).

## Layout

| File | Purpose |
|------|---------|
| `providers.json` | Provider registry: `kind`, URLs, setup type, logo, secrets |
| `locales/<code>.json` | UI strings, per-provider copy, and server error templates for one language |
| `languages.json` | Language picker entries (`code`, `label`, optional `dir`) — add a row per locale file |

## New provider checklist

1. Add a row to `providers.json` (`kind` must match the Rust fetch module name in `src/providers/mod.rs`).
2. Add `providers.<kind>` to **each** locale file under `locales/`.
3. Implement `src/providers/<kind>/` (quota fetch logic stays in code).
4. Optional: `static/icons/<kind>.svg` and route in `routes.rs` if needed.

## UI keys

`ui.*` keys are served at `GET /api/locales/{lang}` for the web UI. Use `{name}` placeholders in JSON; the client substitutes values.

Keep `static/ui-fallback-en.js` in sync with `locales/en.json` → `ui` (instant English before fetch).

If the browser has no saved language (`qb-lang`), the UI asks `GET /api/system-locale`. That is the Windows preferred UI language, or on Linux `LC_ALL`, then `LC_MESSAGES`, then `LANG`. When that language is not shipped, the UI uses the browser language, then English. A language chosen in Settings is what gets stored.

## Error keys

`errors.antigravity.*` templates use `{detail}` / `{code}` placeholders. Server code calls `i18n::format(lang, "antigravity.spawn_failed", &[("detail", …)])`.

## Fallback

Each locale file has `fallback` (`en`). If a provider block is missing in the requested language, the merger uses the English copy. Arabic (`ar`) is `dir: rtl`.