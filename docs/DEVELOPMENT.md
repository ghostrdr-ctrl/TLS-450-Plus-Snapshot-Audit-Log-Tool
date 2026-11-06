# Development notes

A Windows program (Rust, egui) that opens a TLS-450 Plus snapshot
(`snapshot-<serial>-YYMMDD-HHMM.tar`) and lists its events in one table:

- the audit trail (logins, logouts, alarm acknowledgements, setting changes, and the user for each)
- alarm events (posted, acknowledged, cleared, ...)
- static (SLD) leak test starts and ends, manual and automatic

You choose the time range, filter, search, and save what is shown to a `.txt` file.
The saved file is just the rows, with a header line of column names.

## Use

- Run `TLS-450-Plus-Snapshot-Audit-Log-Tool.exe`. If there are `snapshot-*.tar` files next to it, the newest one opens.
  Otherwise use **Open snapshot...** (Ctrl+O), or drag a `.tar` onto the window.
- **Time**: From / To in console local time, as `YYYY-MM-DD`, `YYYY-MM-DD HH:MM` or
  `YYYY-MM-DD HH:MM:SS`. An invalid time, or From after To, is outlined in red and
  shows no rows (so nothing unintended gets saved). **All**, **Last 24 h** and
  **Last 7 days** count back from the newest event in the snapshot.
- **Show**: Audit trail, Alarms, Manual leak tests, Automatic leak tests (off by
  default; there are a lot of them).
- **Sort**: click a column header to sort by it, again to reverse (an arrow shows
  which). Default is Time, newest first. Rows with that cell blank go last;
  numbers inside text sort as numbers (Tank 2 before Tank 10).
- **Filter**: User and Device drop-downs (they list what the ticked event types
  contain), and **Search** (Ctrl+F): any words, case doesn't matter, every word must
  appear somewhere in the row. Esc in the box clears it.
- **Rows**: click to select, Ctrl+click to add, Shift+click for a range, Ctrl+A for
  all shown, Esc to clear. Right-click a row for **Show all events within 15 min**
  (every event type, any user or device, the row stays selected), **Copy row** and
  **Copy row with header** (tab-separated).
- **Save ... to .txt...** (Ctrl+S) and **Copy** use the selected rows if any are
  selected, otherwise every row shown; the buttons say which and how many. The
  file is a header line plus the rows, nothing else.
- Leak tests appear when they end; the console doesn't record who starts them.

## Version

The version is set in one place, `Cargo.toml`. The window title and the exe's
file properties (Properties > Details, publisher Verbose Software) read it.
`docs/CHANGELOG.md` holds the public notes, one `## <version>` section per
release; `build.ps1` refuses to build without one. Tag each release `v<version>`.

Releases are built by GitHub Actions (`.github/workflows/build.yml`): every push
runs the tests on Windows, and a pushed `v<version>` tag also checks the tag
matches `Cargo.toml`, runs `build.ps1` and publishes a release with the exe and
that version's changelog section as the notes. Tags with `-beta.N` / `-rc.N`
become pre-releases.

## Icon

`assets/app.ico` (stamped into the exe by `build.rs`) and `assets/icon-256.png`
(the window icon) are drawn by `tools/make_icon.py` (`python -I tools/make_icon.py`,
needs Pillow). `assets/icon-preview.png` shows every size.

## Build

For an exe to use or hand out, run `build.ps1`. It builds a release with
nothing from this PC inside: crate source paths (which contain the Windows
user folder) are remapped, the PDB reference is left out, and the finished exe
is scanned for the user name and folder paths before it is copied to `dist\`.

```
powershell -ExecutionPolicy Bypass -File build.ps1
```

Output: `dist\TLS-450-Plus-Snapshot-Audit-Log-Tool.exe`.

A plain `cargo build --release` is fine for testing (output in
`C:\build\TLS-450-Plus-Snapshot-Audit-Log-Tool\release\`, set in
`.cargo/config.toml`), but that exe does contain local paths. `cargo test` runs the unit tests;
`TLS450_SNAPSHOT=<file.tar> cargo test -- --ignored --nocapture` also loads a
real snapshot and prints a slice of it.

## Where the data comes from

A snapshot `.tar` holds more `.tgz` files; the program reads:

| File in the snapshot | What it gives |
|---|---|
| `databases.tgz` | The console's SQLite databases (`*.tls`). All times in them are Unix seconds, UTC. |
| `etc.tgz` → `etc/timezone` | The console's zone, e.g. `TZ=":US/Mountain"`. US daylight-saving rules are applied by date. |
| `opt_atg_logs.tgz` → `status.log` | Fallback zone: the last `=== 2026-11-05 15:30:03 -0700 ===` line (early ones can predate the zone being set). |

| Database → table | Used for |
|---|---|
| `AuditTrailHistoryDB` → `AuditTrailHistory` | Audit rows: `TimeStamp`, `Interface`, `Method`, `ID` (device number), `NewValue`, `OldValue`, `User`. |
| `ValidationDB` (and the other `*ValidationDB`) → `FeatureList` + `SettingList` | Names for the audit codes: `Interface` = `FeatureID`, `Method` = `SettingID` (e.g. 1/0 = Tls Security: Login, 12/5 = CSLD Info: Set Csld Climate Factor). |
| `AlarmDB` → `AlarmHistory` + `AlarmHistoryDetail` + `AlarmActionList` | Alarm rows. `AlarmHistory.EventID` = `AlarmHistoryDetail.ItemID`. Tank alarms (`DeviceType` 2) are named from the console's alarm list (1 = LEAK ALARM, ...). The New column holds `PostValue`. |
| `SLDStatusDB` → `SLDHistory` | Leak test rows (finished tests only; a test is saved when it ends). `StartMethod` 1 = manual, 5 = automatic. |
| `TankSetupDB` → `TankSetup` | Tank labels. |
