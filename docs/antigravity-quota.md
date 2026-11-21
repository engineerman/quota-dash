# Antigravity quota

How quota-dash gets an Antigravity number, why that number can go stale, and why T3 Code's own percentage is a different reading.

## The percentage

The live figure is Google's `retrieveUserQuotaSummary`, not T3's cache.

Consumer accounts are read on the daily host first:

`https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary`

The same method on `https://cloudcode-pa.googleapis.com/v1internal` is the second host. quota-dash keeps the first host's value for a bucket id and ignores a later host's value for that same id. The request body needs the Cloud Code project from `loadCodeAssist` (`cloudaicompanionProject`, for these accounts `aicode-consumers`).

Gemini weekly usage is bucket `gemini-weekly`, window `weekly`, field `remainingFraction`. That field is the share left. The card shows the share used:

`(1 - remainingFraction) × 100`, rounded to one decimal. Label: `Gemini · Weekly`.

Claude and GPT share bucket `3p-weekly`. Label: `Claude & GPT (shared) · Weekly`.

On 21 Nov 2026 the daily host returned `remainingFraction` 0.9861472 and 0.9757448 for two consumer accounts (1.4% and 2.4% used). The production host returned `1` for the same `gemini-weekly` bucket, which displays as 0%. A reader that prefers the production host will show a full Gemini quota while the daily host does not.

## The access token

Google's access token lasts about one hour. quota-dash does not mint a new one. It sends whatever access token is already on disk.

For each tracked account, in order:

1. Windows Credential Manager target `gemini:antigravity`, when that record's email is this account. `agy` owns this record. It is one sign-in, not one record per account.
2. quota-dash's own vault, `%LOCALAPPDATA%\quota-dash\secrets`.
3. If that access token is expired, the same email in T3's `antigravity-oauth-token` file, when that copy is still valid. The file is `%USERPROFILE%\.t3\userdata\providers\antigravity\<account id>\antigravity-acp\antigravity-oauth-token`.

A successful read copies the token quota-dash actually used back into the vault. That copy is not a new token.

`agy` has no command that refreshes one chosen account and exits. `agy acp --login` opens a browser and waits for a code, then replaces the single Windows credential. Starting `agy` with no prompt does not write an access token.

T3 can refresh without a model call only when that account's profile contains `acp_token.json` (`client_id`, `client_secret`, `refresh_token`). It then calls `https://oauth2.googleapis.com/token`. With that file absent, T3 uses `antigravity-oauth-token` as-is. Checked 21 Nov 2026: the file was absent, and T3's probe stopped on an expired access token with its own message to start an Antigravity turn, then refresh limits.

A real turn renews the token. T3 starts `agy` for that provider instance and rewrites `antigravity-oauth-token`. The turn also spends quota, so the next reading is a new measurement. From the T3 CLI, with the desktop app already running:

```text
t3 thread start --provider <instance id> --model gemini-3.8-flash --wait --json <project> "<message>"
```

Instance ids for the two Antigravity accounts on this machine are `antigravity` and `antigravity_eelight`.

## Stale card

When the access token is expired and a previous successful reading exists, the card keeps that reading. Status is `stale`. The pill says the quota is not live. The note gives the reading time and says the next Antigravity turn on that account refreshes it. Refresh in the dashboard does not renew the token.

If every window that has a reset time is already past that time, the note says the reading is out of date.

The last successful readings are stored in `%LOCALAPPDATA%\quota-dash\antigravity-last.json`, so a restart can still show them. A card that has never had a successful reading stays on sign-in, with no percentage.

## T3's own cache

quota-dash's T3 card does not call Google. It reads `%USERPROFILE%\.t3\caches\antigravity*.json`. `usageLimits.checkedAt` is when T3 last wrote the file.

That file's "Reported quota" `usedPercent` is not the daily host's `gemini-weekly` fraction. On 21 Nov 2026 a T3 refresh rewrote both account files at 21:14 local and stored 0% for Gemini, while the daily host still reported 1.4% and 2.4% used. Trust `checkedAt` as the time of T3's write. Trust the Antigravity card, not the T3 card, for Gemini weekly usage.
