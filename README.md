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

The snapshot is read on your PC and never changed or sent anywhere. The only thing the program
asks the internet is whether GitHub has a newer version (see Updates).

## Use

1. Download `TLS-450-Plus-Snapshot-Audit-Log-Tool.exe` from [Releases](../../releases).
2. Run it, then open a snapshot `.tar` (or drag one onto the window, or put the exe next to
   your snapshots and the newest opens on its own).

## Updates

At start the program asks GitHub for the latest release. If there is a newer version, a strip
above the toolbar offers it; **Update and restart** downloads it, checks it against its published
SHA-256, replaces the program and reopens the same snapshot. Nothing is downloaded unless you
click it. **Check for updates** (bottom right) checks on demand. If the program's folder can't
be written to, it links to the releases page instead.

## Build

Needs Rust. On Windows: `powershell -ExecutionPolicy Bypass -File build.ps1`
(output in `dist\`). `cargo test` runs the tests. More detail in
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md); changes are in [docs/CHANGELOG.md](docs/CHANGELOG.md).

## Trademarks

TLS-450 Plus and Veeder-Root are trademarks of their owner. This project is independent and not
affiliated with or endorsed by Veeder-Root.

## License

MIT, Copyright (c) 2026 Verbose Software. Free to use, change and share, including
commercially, as long as the copyright and license notice stay with it. See [LICENSE](LICENSE).
