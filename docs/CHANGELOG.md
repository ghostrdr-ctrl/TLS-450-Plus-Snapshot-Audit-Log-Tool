# Changelog

User-facing notes for the TLS-450 Plus Snapshot Audit Log Tool. Everything here is public:
write it for whoever uses the tool, and keep customer and site names, console addresses,
local paths and internal notes out of it.

Style: short. A skimmable list of what changed, one line per change where possible. Plain
punctuation only.

Newest first. Each heading is `## <version>` and must match `version` in `Cargo.toml`.

## 0.2.1

- Open source under the MIT license.
- The exe is now built on GitHub, and each release lists its SHA-256.

## 0.2.0

- Sort by any column: click a column header (Time, Source, Device, User, Event, Value, Previous);
  click it again to reverse. An arrow shows the sort. Rows with that cell blank go last.
  Time sorted newest first is the default and replaces the "Newest first" box.
- Numbers sort as numbers (-166.2 before -84.1, Tank 2 before Tank 10).
- A sort other than the default shows in the status bar and in the Save/Copy hover.
- ALARM and TEST show as small coloured tags.
- The app has its own icon.

## 0.1.1

- Right-click a row and choose "Select all events within 15 min" to select everything around it in
  one step, ready to save.
- Right-clicking inside a selection offers "Copy N selected rows".
- The save dialog suggests a file name with the time span of the rows being saved.
- All columns fit in a narrow window.
- Clearer highlighting: hovered rows, selected rows and the Save/Copy buttons are easier to tell
  apart, and selected rows keep their ALARM/TEST colour.
- Shift+click keeps working after Ctrl+A.

## 0.1.0

- First release. Open a TLS-450 Plus snapshot (.tar) and list its audit trail, alarm events and
  leak test starts and ends in one table, in the console's local time.
- Choose a time range, show or hide each kind of event, filter by user and device, and search.
- Select rows (click, Ctrl+click, Shift+click, Ctrl+A) and save or copy just those, or every row
  shown. The saved .txt is a header line plus the rows.
- Right-click a row to show every event within 15 minutes of it, or to copy it.
- Device and alarm names come from the snapshot itself.
