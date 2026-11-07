# Development notes

A Windows program (Rust, egui) that opens a TLS-450 Plus snapshot
(`snapshot-<serial>-YYMMDD-HHMM.tar`) and lists every log in it in one table, as
17 kinds of event (`src/kind.rs`): the audit trail and log management (with the
user for each), alarm events and the alarm period report, static leak tests
(manual and automatic), CSLD, line leak tests and their diagnostics, compliance
records, deliveries, inventory and shifts, power off and on, the application
log, auto events, the program log files and ISD records.

You choose the time range and kinds, filter, search, and save what is shown to
a `.txt` file. The saved file starts with the snapshot name, the time zone and
the filters, then a header line of column names and the rows.

## Use

- Run `TLS-450-Plus-Snapshot-Audit-Log-Tool.exe`. If there are `snapshot-*.tar` files next to it, the newest one opens.
  Otherwise use **Open snapshot...** (Ctrl+O), or drag a `.tar` onto the window.
- **Time**: From / To in console local time, as `YYYY-MM-DD`, `YYYY-MM-DD HH:MM` or
  `YYYY-MM-DD HH:MM:SS`. An invalid time, or From after To, is outlined in red and
  shows no rows (so nothing unintended gets saved). **All**, **Last 24 h** and
  **Last 7 days** count back from the newest event in the snapshot.
- **Show** (the left panel): a tick box per kind, in groups, with how many rows of
  it match the time range, User, Device and Search ("none": the snapshot has
  none). Defaults ticks the audit trail, alarm events, manual static tests, line
  leak tests, deliveries and power; the bulk records are off. All / None, an
  all/none link per group, right-click or Alt+click a kind to show only it.
  Hide folds the panel (it also folds itself below 1000 px); the Filter row then
  has a Show button. Ticks carry over to the next snapshot opened. Log
  management entries that repeat an audit row are hidden while Audit trail is
  ticked.
- **Sort**: click a column header to sort by it, again to reverse (an arrow shows
  which). Default is Time, newest first. Rows with that cell blank go last;
  numbers inside text sort as numbers (Tank 2 before Tank 10).
- **Filter**: User and Device drop-downs (they list what the ticked kinds contain;
  type to narrow, Enter picks the first match; a choice whose kinds get unticked
  stays chosen and is marked), and **Search** (Ctrl+F): words, case doesn't
  matter, every word must appear somewhere in the row; `"a phrase"` must appear
  as typed within one column; `-word` or `-"a phrase"` leaves rows out. Esc in
  the box clears it.
- The status bar shows how many more rows match in kinds not ticked, and one
  Notices item for anything about how the snapshot was read (tables missing or
  not readable, rows with no time). An empty table says why and offers the fix.
- **Rows**: click to select, Ctrl+click to add, Shift+click for a range, Ctrl+A for
  all shown, Esc to clear. Right-click a row for **Show all events within 15 min**
  (every kind, any user or device, the row stays selected; **Back to previous
  filters** in the toolbar undoes it), **Copy row** and
  **Copy row with header** (tab-separated).
- **Save ... to .txt...** (Ctrl+S) and **Copy** use the selected rows if any are
  selected, otherwise every row shown; the buttons say which and how many. The
  file starts with the snapshot, time zone and filters; Copy is the rows only.
- The console doesn't record who starts a leak test.

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

The in-app updater (`src/update.rs`) reads
`https://api.github.com/repos/ghostrdr-ctrl/TLS-450-Plus-Snapshot-Audit-Log-Tool/releases/latest`
and installs only an asset named exactly `TLS-450-Plus-Snapshot-Audit-Log-Tool.exe` from that
repo's release downloads, with a SHA-256 (GitHub's asset digest, or the line the workflow writes
in the notes). Keep the asset name and that notes line when changing the workflow.
Pre-releases are never offered.

## Before a release

The tests and screenshots don't click buttons or open other programs. Before tagging, run the
built exe on Windows and check each of these by hand:

- [ ] A snapshot opens: next to the exe at start, with **Open snapshot...**, and dragged onto the window.
- [ ] The Store row shows the store; **Copy store info** pastes into Notepad.
- [ ] **Save ... to .txt** writes a file that opens in Notepad and starts with the Store, Snapshot and
      filter lines.
- [ ] **Copy ... rows** pastes into Notepad, and into Excel as columns.
- [ ] Right-click a row, **Show all events within 15 min**, then **Back to previous filters**.
- [ ] **Check for updates** says it is up to date (or offers the newer release).
- [ ] With an older exe: the update strip appears, **What's new** shows the notes, the link in it
      opens the release page in the browser, and **Update and restart** replaces the exe and starts it.

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
| `opt_atg_distinfo.tgz` → `filelist.md5` | The console software, from its first lines (`# Release: 11.D`, `# Version: 400.15`). Only the first 4 KB is read. |

| Database → table | Used for |
|---|---|
| `AuditTrailHistoryDB` → `AuditTrailHistory` | Audit rows: `TimeStamp`, `Interface`, `Method`, `ID` (device number), `NewValue`, `OldValue`, `User`. |
| `ValidationDB` (and the other `*ValidationDB`) → `FeatureList` + `SettingList` | Names for the audit codes: `Interface` = `FeatureID`, `Method` = `SettingID` (e.g. 1/0 = Tls Security: Login, 12/5 = CSLD Info: Set Csld Climate Factor). |
| `AlarmDB` → `AlarmHistory` + `AlarmHistoryDetail` + `AlarmActionList` | Alarm rows. `AlarmHistory.EventID` = `AlarmHistoryDetail.ItemID`. Tank alarms (`DeviceType` 2) are named from the console's alarm list (1 = LEAK ALARM, ...). The New column holds `PostValue`. |
| `SLDStatusDB` → `SLDHistory` | Static leak tests, a row at the start and one at the end (finished tests only; a test is saved when it ends). `StartMethod` 1 = manual, 5 = automatic. `TestType` 0/1/2 = 0.2 gph / 0.1 gph / gross; `LeakTestResult_1/_2/_3` hold that test's result (WSDL `LeakTestResult`). |
| `TankSetupDB` → `TankSetup`, `LineSetupDB` → `LineSetup` | Tank and line labels (a PLLD `AnalyzerID` is its line number). |
| `SiteStatusDB` → `HeaderSetup` | The store: `Header1` to `Header4` (the report header set up on the console; blank lines left out). The console serial number comes from the snapshot file name. |

The other kinds are read by `src/snapshot/history.rs`, one function per table, with `select *` and
columns looked up by name (a table that is absent gives no rows; one missing a needed column is
reported in the status bar Notices). Codes are named from the WSDL enumerations noted in the code.

| Kind | Tables |
|---|---|
| Log management | `LogManagementHistoryDB.LogManagementHistory` (audit layout; exact repeats of an audit row are flagged) |
| Alarm period report | `AlarmDB.AlarmPeriodNormalHistory` |
| CSLD | `CSLDStatusDB`: `CSLDRateRecordHistory` (at each test's end), `CSLDStateChangesHistory`, `CSLDPosRejectStatusChangesHistory`, `CSLDTestStatus` |
| Line leak tests | `PLLDInternalsHistoriesDB.UserResultHistory` |
| Line leak diagnostics | `PLLDInternalsHistoriesDB`: `ResultHistory`, `PrecisionDiagHistory`, `PortDiagnosticHistory`, `NoSpikeDiagnosticHistory`, `AutoConfirmHistory` |
| Compliance records | `ComplianceHistoryDB`: `MonthlyComplianceHistory`, `LastPassedComplianceHistory`, `TidelLeakResult`; `PLLDInternalsRecordsDB.LastPassedTest` |
| Deliveries | `DeliveryDB`: `StdDeliveryHist`, `PowerOutageDeliveryHistory`, `TidelDeliveryClose` |
| Inventory and shifts | `InventoryHistory` and `ShiftInvHistory` in `DeliveryHistoryDB` and `InventoryHistoryDB`; `PowerOutageHistory`, `GeneratorInventoryReport` |
| Power off and on | `ApplicationLogDB.PowerResetHistory` |
| Application log | `ApplicationLogDB.ApplicationLogHistory` |
| Auto events | `AutoEventsDB.AutoEventsLog` |
| ISD records | `ISDQListDB.ISD_pvcrack_history` |
| Program log files | `opt_atg_logs.tgz`: every file under `logs/` except `status.log`, rotated `.gz` copies unpacked. Dated lines are rows (console local time as written); undated lines continue the row above. |

Not read: `ManifoldedDeliveryHist` and `PLLDTestHistory` (repeat other tables), `CSLDVolumeHistory`,
`HourlyPassed_3_0_Tests` and `ProfileTestResult` (counters with no event time). A time of 0 means
the console never set it; those rows are counted in Notices, not listed.
