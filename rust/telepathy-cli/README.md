# telepathy-cli

`telepathy-cli` is a machine-oriented adapter around `telepathy_core::native::NativeTelepathy`.

It reads newline-delimited JSON commands from `stdin` and writes newline-delimited JSON
output lines to `stdout` (one complete JSON object per line).

For the full protocol specification (startup, command and event schemas, ack/result behavior,
lifecycle, and attachment encoding details), see:

- [`docs/CLI.md`](../../docs/CLI.md)

All diagnostics/logging are emitted to `stderr` so `stdout` stays protocol-clean.

## Startup discovery

Optional iroh discovery can be configured at startup:

- `--relay-url` / `TELEPATHY_RELAY_URL` — HTTP relay URL (for example `http://10.0.10.1:3340`)
- `--dns-endpoint` / `TELEPATHY_DNS_ENDPOINT` — DNS resolver host:port (for example `10.0.10.1:5300`)
- `--pkarr-relay` / `TELEPATHY_PKARR_RELAY` — pkarr relay URL (for example `http://10.0.10.1:8080/pkarr`)

CLI flags take precedence over environment variables when both are set.
