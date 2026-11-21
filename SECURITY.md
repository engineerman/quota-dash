# Security

## Reporting a vulnerability

Open a private GitHub security advisory on this repository once it is public. Do not put API keys, tokens, session files, or the contents of `quota-dash.toml` in an issue, pull request, or screenshot.

## What the process can read

quota-dash reads local sign-ins and API keys so it can show quotas for the coding agents installed on this PC. Those values stay on the machine. The server does not log them.

## Network

The default listen address is `127.0.0.1`. Keep it there. A wider bind, such as `0.0.0.0`, lets other machines on the network call the API and read the same quota report, including account labels.

## Config and screenshots

Store keys in the gitignored `quota-dash.toml`, or in `OPENROUTER_API_KEY` for OpenRouter. `config.example.toml` is the file that belongs in git.

Screenshots for the docs and for issues need account names, email addresses, and balances blurred or removed before they are published.
