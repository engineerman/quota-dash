# Contributing

## Checks

```powershell
cargo test
```

For a UI change, run the app and click through the screen you touched:

```powershell
cargo run --release
```

Then open http://127.0.0.1:8787/.

## Copy and providers

UI strings and provider blurbs live in `data/`. See [data/README.md](data/README.md). A new provider needs a row in `data/providers.json`, a block in every `data/locales/*.json`, and a fetch module under `src/providers/`.

## Secrets

`quota-dash.toml` is local. Leave it out of commits. The same goes for API keys, tokens, and files under `runs/`.

Screenshots that go into the repo or an issue need account names, email addresses, and balances blurred or removed.
