# TLS-450 Plus Snapshot Audit Log Tool

A small Windows program that opens a TLS-450 Plus snapshot (`snapshot-<serial>-YYMMDD-HHMM.tar`)
and shows its audit trail, alarm events and static leak tests in one table, in the console's
local time.

![App icon](assets/icon-256.png)

## What it does

- Lists logins, logouts, setting changes and alarm acknowledgements with the user for each,
  alarm events, and manual and automatic leak tests.
- Pick a time range, show or hide each kind of event, filter by user or device, and search.
- Sort by any column, select rows, and save or copy them as plain text.
- Right-click a row to see everything that happened within 15 minutes of it.

Nothing is sent anywhere: the snapshot is read on your PC and never changed.

## Use

1. Download `TLS-450-Plus-Snapshot-Audit-Log-Tool.exe` from [Releases](../../releases).
2. Run it, then open a snapshot `.tar` (or drag one onto the window, or put the exe next to
   your snapshots and the newest opens on its own).

## Build

Needs Rust. On Windows: `powershell -ExecutionPolicy Bypass -File build.ps1`
(output in `dist\`). `cargo test` runs the tests. More detail in
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md); changes are in [docs/CHANGELOG.md](docs/CHANGELOG.md).

## License

MIT, Copyright (c) 2026 Verbose Software. Free to use, change and share, including
commercially, as long as the copyright and license notice stay with it. See [LICENSE](LICENSE).
