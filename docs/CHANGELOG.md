# Changelog

User-facing notes for the TLS-450 Plus Snapshot Audit Log Tool. Everything here is public:
write it for whoever uses the tool, and keep customer and site names, console addresses,
local paths and internal notes out of it.

Style: short. A skimmable list of what changed, one line per change where possible. Plain
punctuation only.

Newest first. Each heading is `## <version>` and must match `version` in `Cargo.toml`.

## 0.5.0

- The store the snapshot is from: its header (name, address) as set up on the console, and the
  console's serial number and software, at the top of the window and of a saved .txt.
- "What's new" now shows the release notes in a window. It and "Open download page" did
  nothing before, because the program could not open web pages.

## 0.4.0

- Every log in the snapshot, not just the audit trail, alarms and static leak tests: log
  management, the alarm period report, CSLD, line leak (PLLD) tests and their diagnostics,
  compliance records, deliveries, inventory and shift closes, power off and on, the application
  log, auto events, ISD records and the console's program log files. 17 kinds in all.
- A Show panel on the left: a tick box for each kind, in groups, with how many rows of each match
  the other filters. Defaults, All and None buttons, an all/none link per group, and right-click
  or Alt+click to show only one kind. Hide folds it away; it folds itself on a narrow window.
- Log management entries that repeat an audit row are shown once while the audit trail is ticked.
- Static leak tests show their test type, start volume and result.
- Search: "quoted phrases" and -words to leave rows out.
- User and Device lists have a box to narrow them as you type. A choice is kept when its kinds are
  unticked, and marked.
- The status bar says how many more rows match in kinds not ticked; an empty table says why and
  offers the fix.
- "Back to previous filters" after "Show all events within 15 min".
- A saved .txt starts with the snapshot name, time zone and the filters used.
- Rows the console saved without a time are counted in the status bar.
- Each kind has its own tag color.
- Tank alarms the console added later (low temperature, gain and loss warnings, ...) are named.
- A damaged or oversized snapshot is refused with a message instead of hanging at "Loading".
- Time boxes refuse negative times.

## 0.3.1

- First release published on GitHub, including the update check from 0.3.0.

## 0.3.0

- Updates: at start the program checks GitHub for a newer version and offers it in a strip above
  the toolbar. "Update and restart" downloads it, checks its SHA-256, replaces the program and
  reopens the same snapshot. Nothing is downloaded unless you click it.
- "Check for updates" at the bottom right checks on demand.

## 0.2.1

- Open source under the MIT license.
- The exe is now built on GitHub, and each release lists its SHA-256.

## 0.2.0

- Sort by any column: click a column header (Time, Source, Device, User, Event, Value, Previous);
  click it again to reverse. An arrow shows the sort. Rows with that cell blank go last.
  Time sorted newest first is the default and replaces the "Newest first" box.
- Numbers sort as numbers (-166.2 before -84.1, Tank 2 before Tank 10).
- A sort other than the default shows in the status bar and in the Save/Copy hover.
- ALARM and TEST show as small colored tags.
- The app has its own icon.

## 0.1.1

- Right-click a row and choose "Select all events within 15 min" to select everything around it in
  one step, ready to save.
- Right-clicking inside a selection offers "Copy N selected rows".
- The save dialog suggests a file name with the time span of the rows being saved.
- All columns fit in a narrow window.
- Clearer highlighting: hovered rows, selected rows and the Save/Copy buttons are easier to tell
  apart, and selected rows keep their ALARM/TEST color.
- Shift+click keeps working after Ctrl+A.

## 0.1.0

- First release. Open a TLS-450 Plus snapshot (.tar) and list its audit trail, alarm events and
  leak test starts and ends in one table, in the console's local time.
- Choose a time range, show or hide each kind of event, filter by user and device, and search.
- Select rows (click, Ctrl+click, Shift+click, Ctrl+A) and save or copy just those, or every row
  shown. The saved .txt is a header line plus the rows.
- Right-click a row to show every event within 15 minutes of it, or to copy it.
- Device and alarm names come from the snapshot itself.
