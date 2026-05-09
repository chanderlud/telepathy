# telepathy-cli

`telepathy-cli` is a machine-oriented adapter around `telepathy_core::native::NativeTelepathy`.

It reads newline-delimited JSON commands from `stdin` and writes newline-delimited JSON
output lines to `stdout` (one complete JSON object per line).

For the full protocol specification (startup, command and event schemas, ack/result behavior,
lifecycle, and attachment encoding details), see:

- [`docs/CLI.md`](../../docs/CLI.md)

All diagnostics/logging are emitted to `stderr` so `stdout` stays protocol-clean.
