//! Reads a TLS-450 Plus snapshot .tar into a flat list of events, one `Kind`
//! per log: the audit trail and alarms here, everything else in `history`.
//!
//! Where it reads from (see docs/DEVELOPMENT.md):
//! - databases.tgz: the console's SQLite `.tls` files (times are Unix seconds, UTC).
//! - etc.tgz > etc/timezone: the console's time zone (`TZ=":US/Mountain"`);
//!   status.log's `=== date time -0700 ===` lines are the fallback.
//! - opt_atg_etc.tgz > TLSWebService.wsdl: the console's own names for device
//!   types and alarm types (`<enumeration value="..."/><!-- enum const = N -->`).
//! - opt_atg_logs.tgz: the console's program log files (`history::program_logs`).

mod history;

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use rusqlite::{Connection, OpenFlags};

pub use crate::kind::Kind;

/// Tank alarm labels by AlarmID (DeviceType 2), as the console's screens name them.
const TANK_ALARMS: [&str; 33] = [
    "SETUP DATA WARNING",
    "LEAK ALARM",
    "HIGH WATER ALARM",
    "OVERFILL ALARM",
    "LOW PRODUCT ALARM",
    "SUDDEN LOSS ALARM",
    "HIGH PRODUCT ALARM",
    "INVALID FUEL LEVEL",
    "PROBE OUT",
    "HIGH WATER WARNING",
    "DELIVERY NEEDED",
    "MAX PRODUCT ALARM",
    "GROSS TEST FAIL",
    "PERIODIC TEST FAIL",
    "ANNUAL TEST FAIL",
    "PER TST NEEDED WRN",
    "ANN TST NEEDED WRN",
    "PER TST NEEDED ALM",
    "ANN TST NEEDED ALM",
    "TANK TEST ACTIVE",
    "NO CSLD IDLE TIME",
    "TANK SIPHON BREAK",
    "CSLD RATE INCR WARN",
    "ACCU_CHART CAL WARN",
    "RECON WARNING",
    "RECON ALARM",
    "LOW TEMP WARNING",
    "MISSING TICKET WARN",
    "GROSS FAIL LINE TNK",
    "DELIVY DENSITY WARN",
    "DENSITY WARNING",
    "FUEL QUAL ALM",
    "HIGH TEMPERATURE ALARM",
];

/// Device type (WSDL `DeviceType` name, without `DEVTYPE`) -> (shown name, WSDL alarm list).
const DEVICE_TYPES: &[(&str, &str, &str)] = &[
    ("SYSTEM", "System", "SystemAlarms"),
    ("TANK", "Tank", "InTankAlarms"),
    ("LIQUID", "Liquid sensor", "LiquidSensorAlarms"),
    ("VAPOR", "Vapor sensor", "VaporSensorAlarms"),
    ("INPUT", "Input", "InputAlarms"),
    ("GRNDH2O", "Groundwater sensor", "GndWaterSensorAlarms"),
    ("RELAY", "Relay", "RelayAlarms"),
    ("PLLDSNS", "PLLD line", "PLLDAlarms"),
    ("PUMP", "Pump", "PumpAlarms"),
    ("LINE", "Line", "LineAlarms"),
    ("MODBUS", "Modbus", "ModBusAlarms"),
    ("ISD", "ISD", "ISDAlarms"),
    ("HOSE", "Hose", "HoseAlarms"),
    ("PRINTER", "Printer", "PrinterAlarms"),
    ("COMM", "Comm port", "CommunicationAlarms"),
    ("CONTACT", "Contact", "ContactAlarms"),
    ("AUTOEVENT", "Auto event", "AutoEventAlarms"),
    ("MAGSUMPSENSOR", "MAG sump sensor", "MAGSensorAlarms"),
    ("VACSENSOR", "Vacuum sensor", "VacSensorAlarms"),
    ("LINEPRESSURESENSOR", "Line pressure sensor", "LPRSensorAlarms"),
    ("VAPORVALVE", "Vapor valve", "VaporValveAlarms"),
    ("SSM", "SSM", "SSMAlarms"),
    ("UPS", "UPS", "UPSAlarms"),
    ("VMC", "VMC", "VMCAlarms"),
    ("TEMPERATURE", "Temperature sensor", "TempSensorAlarms"),
    ("TEMPCONTROL", "Temperature control", "TempControlAlarms"),
    ("RECIRCULATION", "Recirculation", "RecirculationAlarms"),
    ("APM", "APM", "ApmAlarms"),
];

/// Words that are acronyms on the console, for audit names.
const ACRONYMS: &[&str] = &[
    "TLS", "CSLD", "SLD", "PLLD", "WPLLD", "DPLLD", "TCP", "IP", "DNS", "SMTP", "NTP", "BIR", "DVM", "HRM", "IFSF", "USB", "URL", "VMC",
    "VPM", "ISD", "ISFC", "ATG", "LPR", "MAG", "ID", "PC", "UPS", "SSM", "APM", "HC", "RVP", "MQTT", "DHCP", "GMT", "UTC", "PMC", "VFM",
    "FMS", "SNMP", "HTTP", "HTTPS", "FTP", "PDF", "CSV", "XML", "LED", "LCD", "UI", "AC", "DC",
];

#[derive(Clone, Debug, Default)]
pub struct Row {
    pub utc: i64,
    /// Console local time, `YYYY-MM-DD HH:MM:SS`.
    pub time: String,
    pub kind: Kind,
    pub device: String,
    pub user: String,
    pub event: String,
    pub value: String,
    pub previous: String,
    /// A Log Management entry that is also in the audit trail: hidden while
    /// audit rows are shown, so the same action is not listed twice.
    pub repeat: bool,
}

impl Row {
    pub fn columns(&self) -> [&str; 7] {
        [
            &self.time,
            self.kind.tag(),
            &self.device,
            &self.user,
            &self.event,
            &self.value,
            &self.previous,
        ]
    }
    /// Lower-case text of every column, for search. Columns are joined by a
    /// line break (which the search box can't hold), so a phrase never runs
    /// from one column into the next.
    pub fn haystack(&self) -> String {
        self.columns().join("\n").to_lowercase()
    }
}

pub struct Loaded {
    pub path: PathBuf,
    pub clock: Clock,
    pub rows: Vec<Row>,
    /// Parts of the snapshot that could not be read (e.g. "audit trail").
    pub missing: Vec<String>,
    /// Tables that are in the snapshot but could not be read (an unexpected layout).
    pub unreadable: Vec<String>,
    /// Rows that were in a table but couldn't be read (e.g. a NULL time).
    pub skipped: usize,
    /// Rows the console saved with no time, by `Kind::index`: not shown.
    pub no_time: [usize; Kind::COUNT],
}

/// What the readers share: the clock and the console's names for its devices.
pub struct Ctx {
    pub clock: Clock,
    tanks: HashMap<i64, String>,
    lines: HashMap<i64, String>,
    enums: HashMap<String, BTreeMap<i64, String>>,
    alarm_labels: HashMap<(i64, i64), String>,
    /// Audit Interface/Method codes -> names.
    pub names: HashMap<(i64, i64), (String, String)>,
}

impl Ctx {
    pub fn tank(&self, id: i64) -> String {
        match self.tanks.get(&id) {
            Some(l) if !l.is_empty() => format!("Tank {id} {l}"),
            _ => format!("Tank {id}"),
        }
    }

    pub fn line(&self, id: i64) -> String {
        match self.lines.get(&id) {
            Some(l) if !l.is_empty() => format!("Line {id} {l}"),
            _ => format!("Line {id}"),
        }
    }

    /// A WSDL enumeration's name for a value (`CsldTestResult`, 1 -> `RateTestPass`).
    pub fn enum_name(&self, ty: &str, v: i64) -> Option<&str> {
        self.enums.get(ty).and_then(|e| e.get(&v)).map(String::as_str)
    }

    /// The WSDL device type name without `DEVTYPE` (2 -> "TANK").
    fn wsdl_type(&self, dtype: i64) -> Option<String> {
        self.enum_name("DeviceType", dtype)
            .map(|s| s.trim_end_matches("DEVTYPE").to_string())
    }

    /// A device by its type and number, as the alarm list names it
    /// ("Tank 1 REGULAR", "Liquid sensor 3 STP SUMP", "System").
    pub fn device(&self, dtype: i64, did: i64) -> String {
        if dtype == 2 {
            return self.tank(did);
        }
        let wsdl = self.wsdl_type(dtype);
        let known = wsdl.as_deref().and_then(|w| DEVICE_TYPES.iter().find(|d| d.0 == w));
        let type_name = match (known, &wsdl) {
            (Some(k), _) => k.1.to_string(),
            (None, Some(w)) => title_case(w),
            (None, None) => format!("Device type {dtype}"),
        };
        if type_name == "Line" {
            return self.line(did);
        }
        let mut device = if dtype == 1 { type_name } else { format!("{type_name} {did}") };
        if let Some(l) = self.alarm_labels.get(&(dtype, did)).filter(|l| !l.is_empty()) {
            device = format!("{device} {l}");
        }
        device
    }

    /// An alarm's name from its device type and alarm number.
    pub fn alarm_name(&self, dtype: i64, aid: i64) -> String {
        if dtype == 2 {
            // The console's own screen names first; newer alarms from its WSDL.
            return usize::try_from(aid)
                .ok()
                .and_then(|i| TANK_ALARMS.get(i))
                .map(|s| s.to_string())
                .or_else(|| {
                    self.enums
                        .get("InTankAlarms")
                        .and_then(|e| e.get(&aid))
                        .map(|s| wsdl_alarm_label(s))
                })
                .unwrap_or(format!("TANK ALARM {aid}"));
        }
        self.wsdl_type(dtype)
            .and_then(|w| DEVICE_TYPES.iter().find(|d| d.0 == w))
            .and_then(|k| self.enums.get(k.2))
            .and_then(|e| e.get(&aid))
            .map(|s| wsdl_alarm_label(s))
            .unwrap_or(format!("ALARM {aid}"))
    }

    /// An audit (or Log Management) entry's Device and Event columns.
    pub fn audit_event(&self, iface: i64, method: i64, id: i64) -> (String, String) {
        let (iname, mname) = self
            .names
            .get(&(iface, method))
            .cloned()
            .unwrap_or((format!("Interface{iface}"), format!("Method{method}")));
        let tankish = ["Tank", "SLD", "CSLD", "Probe"].iter().any(|k| iname.contains(k));
        let device = if id <= 0 {
            String::new()
        } else if tankish {
            self.tank(id)
        } else {
            format!("{} {id}", words(&iname).trim_end_matches(" Info"))
        };
        (device, format!("{}: {}", words(&iname), words(&mname)))
    }
}

/// Rows as the readers make them; a row with a time that isn't one is
/// counted, not shown.
#[derive(Default)]
pub struct Out {
    pub rows: Vec<Row>,
    pub skipped: usize,
    pub unreadable: Vec<String>,
    /// Rows the console saved with no time (0), by `Kind::index`.
    pub no_time: [usize; Kind::COUNT],
}

/// Earliest and latest believable event times (2000 to 2100). A time of 0 is
/// one the console never set (e.g. a Log Management entry written at power up
/// before the clock ran): counted, not shown. Anything else outside the range
/// is a damaged row.
const TIME_RANGE: std::ops::RangeInclusive<i64> = 946_684_800..=4_102_444_800;

impl Out {
    /// Adds `r` with its local time filled in, if its time is believable.
    pub fn push(&mut self, ctx: &Ctx, utc: Option<i64>, mut r: Row) {
        match utc {
            Some(0) => self.no_time[r.kind.index()] += 1,
            Some(t) if TIME_RANGE.contains(&t) => {
                r.utc = t;
                r.time = ctx.clock.local(t);
                self.rows.push(r);
            }
            _ => self.skipped += 1,
        }
    }
}

pub fn load(tar_path: &Path) -> Result<Loaded, String> {
    SKIPPED.with(|s| s.set(0));
    let ex = Extracted::from_tar(tar_path)?;
    let clock = if let Some(c) = ex.timezone.as_deref().and_then(clock_from_timezone) {
        c
    } else if let Some(off) = ex.status_log.as_deref().and_then(offset_from_status) {
        Clock {
            std: off,
            dst: false,
            name: hm(off),
            fixed: true,
        }
    } else {
        Clock {
            std: 0,
            dst: false,
            name: "UTC".into(),
            fixed: true,
        }
    };
    let labels = |db: &str, sql: &str| -> HashMap<i64, String> {
        lookup(&ex.db(db), sql, |r| Ok((r.get::<_, i64>(0)?, clean(&text(r, 1)))))
            .unwrap_or_default()
            .into_iter()
            .collect()
    };
    let alarms = ex.db("AlarmDB.tls");
    let ctx = Ctx {
        clock,
        tanks: labels(
            "TankSetupDB.tls",
            "select DeviceID, DeviceLabel from TankSetup where Configured = 1",
        ),
        lines: labels(
            "LineSetupDB.tls",
            "select DeviceID, DeviceLabel from LineSetup where Configured = 1",
        ),
        enums: ex.wsdl.as_deref().map(parse_wsdl_enums).unwrap_or_default(),
        alarm_labels: lookup(&alarms, "select DeviceType, DeviceID, DeviceLabel from AlarmDevice", |r| {
            Ok(((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?), clean(&text(r, 2))))
        })
        .unwrap_or_default()
        .into_iter()
        .collect(),
        names: audit_names(&ex),
    };
    let mut missing = vec![];
    let mut out = Out::default();

    // Audit trail.
    match query(
        &ex.db("AuditTrailHistoryDB.tls"),
        "select TimeStamp, Interface, Method, ID, NewValue, OldValue, User from AuditTrailHistory order by ItemID",
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3).unwrap_or(-1),
                text(r, 4),
                text(r, 5),
                text(r, 6),
            ))
        },
    ) {
        None => missing.push("audit trail".to_string()),
        Some(list) => {
            for (at, i, m, id, new, old, user) in list {
                let (device, event) = ctx.audit_event(i, m, id);
                out.push(
                    &ctx,
                    Some(at),
                    Row {
                        kind: Kind::Audit,
                        device,
                        user: clean(&user),
                        event,
                        value: clean(&new),
                        previous: clean(&old),
                        ..Row::default()
                    },
                );
            }
        }
    }

    // Alarms.
    let actions: HashMap<i64, String> = lookup(&alarms, "select ID, Name from AlarmActionList", |r| {
        Ok((r.get::<_, i64>(0)?, text(r, 1)))
    })
    .unwrap_or_default()
    .into_iter()
    .map(|(i, n)| {
        let n = match n.as_str() {
            "POST_ALARM" => "posted",
            "ACK_ALARM" => "acknowledged",
            "AUTH_ACK_ALARM" => "acknowledged (authorized)",
            "OVERRIDE_ALARM" => "overridden",
            "CLEAR_ALARM" => "cleared",
            "SUSPEND_ALARM" => "suspended",
            "REPEAT_ALARM" => "re-posted",
            "UPGRADE_ALARM" => "upgraded",
            "OVERRIDE_CLEARED" => "override cleared",
            other => other,
        }
        .to_string();
        (i, n)
    })
    .collect();
    match query(
        &alarms,
        "select h.TimeStamp, h.EventType, d.DeviceType, d.DeviceID, d.AlarmID, d.PostValue \
         from AlarmHistory h join AlarmHistoryDetail d on d.ItemID = h.EventID order by h.ItemID",
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, f64>(5).ok(),
            ))
        },
    ) {
        None => missing.push("alarms".to_string()),
        Some(list) => {
            for (at, et, dtype, did, aid, val) in list {
                let action = actions.get(&et).cloned().unwrap_or(format!("event {et}"));
                out.push(
                    &ctx,
                    Some(at),
                    Row {
                        kind: Kind::Alarm,
                        device: ctx.device(dtype, did),
                        event: format!("{} {action}", ctx.alarm_name(dtype, aid)),
                        value: val.map(|v| format!("{v:.1}")).unwrap_or_default(),
                        ..Row::default()
                    },
                );
            }
        }
    }

    // Static (SLD) leak tests: one row at the start, one at the end.
    if !history::static_tests(&ex, &ctx, &mut out) {
        missing.push("static leak tests".to_string());
    }

    // Everything else the console keeps; a site without a feature has no rows.
    history::read_all(&ex, &ctx, &mut out);

    let mut rows = out.rows;
    rows.sort_by_key(|r| r.utc);
    Ok(Loaded {
        path: tar_path.to_path_buf(),
        clock: ctx.clock,
        rows,
        missing,
        unreadable: out.unreadable,
        skipped: out.skipped + SKIPPED.with(|s| s.get()),
        no_time: out.no_time,
    })
}

// ------------------------------------------------------------- extraction --

struct Extracted {
    dir: PathBuf,
    status_log: Option<String>,
    timezone: Option<String>,
    wsdl: Option<String>,
    /// Program log files (opt/atg/logs, not status.log): (file name, text),
    /// rotated `.gz` copies unpacked.
    logs: Vec<(String, String)>,
}

/// A program log file bigger than this (unpacked) is not read: the table
/// would hold more noise than anyone can read. All of them together are
/// capped too.
const MAX_LOG_BYTES: u64 = 32 * 1024 * 1024;
const MAX_LOGS_TOTAL: u64 = 256 * 1024 * 1024;
/// One console database, and all of them, unpacked (real ones are a few MB).
const MAX_DB_BYTES: u64 = 512 * 1024 * 1024;
const MAX_DB_TOTAL: u64 = 2 * 1024 * 1024 * 1024;
/// status.log and the WSDL.
const MAX_TEXT_BYTES: u64 = 64 * 1024 * 1024;

impl Drop for Extracted {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

impl Extracted {
    fn from_tar(tar_path: &Path) -> Result<Extracted, String> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("tls450log-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut ex = Extracted {
            dir,
            status_log: None,
            timezone: None,
            wsdl: None,
            logs: vec![],
        };

        let f = fs::File::open(tar_path).map_err(|e| format!("Can't open {}: {e}", tar_path.display()))?;
        let mut outer = tar::Archive::new(f);
        let mut found_db = false;
        let (mut db_bytes, mut log_bytes) = (0u64, 0u64);
        let entries = outer.entries().map_err(|_| "This file is not a snapshot .tar.".to_string())?;
        for entry in entries {
            let mut entry = entry.map_err(|_| "This file is not a snapshot .tar (or it is damaged).".to_string())?;
            let name = base_name(&entry.path().map_err(|e| e.to_string())?.to_string_lossy());
            if !matches!(name.as_str(), "databases.tgz" | "opt_atg_logs.tgz" | "etc.tgz" | "opt_atg_etc.tgz") {
                continue;
            }
            let mut inner = tar::Archive::new(GzDecoder::new(&mut entry));
            let Ok(inner_entries) = inner.entries() else { continue };
            for e in inner_entries {
                let Ok(mut e) = e else { continue };
                let Ok(p) = e.path() else { continue };
                let full = p.to_string_lossy().replace('\\', "/");
                let n = base_name(&full);
                let rel = full.trim_start_matches("./").trim_start_matches('/');
                if name == "databases.tgz" && n.ends_with(".tls") {
                    // Only a plain file name is written, and only inside our own
                    // folder: no drive ("C:x.tls"), no path, no odd characters.
                    if !safe_db_name(&n) || !e.header().entry_type().is_file() {
                        continue;
                    }
                    let path = ex.dir.join(&n);
                    if path.parent() != Some(ex.dir.as_path()) {
                        continue;
                    }
                    // Streamed to disk with a cap, so a small archive can't unpack
                    // into gigabytes.
                    let room = MAX_DB_TOTAL.saturating_sub(db_bytes).min(MAX_DB_BYTES);
                    let mut out = fs::File::create(&path).map_err(|e| e.to_string())?;
                    let n_bytes = std::io::copy(&mut (&mut e).take(room + 1), &mut out).map_err(|e| e.to_string())?;
                    drop(out);
                    if n_bytes > room {
                        let _ = fs::remove_file(&path);
                        return Err(format!(
                            "{n} in this snapshot is too large to open (over {} MB).",
                            room / (1024 * 1024)
                        ));
                    }
                    db_bytes += n_bytes;
                    found_db = true;
                } else if name == "opt_atg_logs.tgz" && n == "status.log" {
                    ex.status_log = read_capped(&mut e, MAX_TEXT_BYTES);
                } else if name == "opt_atg_logs.tgz"
                    && e.header().entry_type().is_file()
                    && !n.starts_with("status.log")
                    && rel.contains("logs/")
                {
                    let room = MAX_LOGS_TOTAL.saturating_sub(log_bytes).min(MAX_LOG_BYTES);
                    let text = if n.ends_with(".gz") {
                        read_capped(&mut GzDecoder::new(&mut e), room)
                    } else {
                        read_capped(&mut e, room)
                    };
                    if let Some(t) = text {
                        log_bytes += t.len() as u64;
                        ex.logs.push((n.clone(), t));
                    }
                } else if name == "etc.tgz" && rel == "etc/timezone" {
                    ex.timezone = read_capped(&mut e, 4096);
                } else if name == "opt_atg_etc.tgz" && n == "TLSWebService.wsdl" {
                    ex.wsdl = read_capped(&mut e, MAX_TEXT_BYTES);
                }
            }
        }
        if !found_db {
            return Err("This .tar has no console databases (databases.tgz). Is it a TLS-450 Plus snapshot?".into());
        }
        Ok(ex)
    }

    fn db(&self, name: &str) -> Option<Connection> {
        let p = self.dir.join(name);
        p.exists()
            .then(|| Connection::open_with_flags(p, OpenFlags::SQLITE_OPEN_READ_ONLY).ok())
            .flatten()
    }
}

/// A text file of at most `cap` bytes; None if it is bigger or can't be read.
fn read_capped(e: &mut impl Read, cap: u64) -> Option<String> {
    let mut buf = vec![];
    e.take(cap + 1).read_to_end(&mut buf).ok()?;
    if buf.len() as u64 > cap {
        return None;
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// A database file name as the console writes them ("AlarmDB.tls"): letters,
/// digits, `_`, `-` and `.` only, so it can only name a file in our folder.
fn safe_db_name(n: &str) -> bool {
    n.len() <= 100 && !n.starts_with('.') && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn base_name(p: &str) -> String {
    p.rsplit(['/', '\\']).next().unwrap_or(p).to_string()
}

/// Runs a query. None when the database or table isn't there (or the query
/// fails), so the caller can say which part is missing.
fn query<T>(c: &Option<Connection>, sql: &str, f: impl Fn(&rusqlite::Row) -> rusqlite::Result<T>) -> Option<Vec<T>> {
    let c = c.as_ref()?;
    let mut st = c.prepare(sql).ok()?;
    let it = st.query_map([], |r| f(r)).ok()?;
    let mut out = vec![];
    for r in it {
        match r {
            Ok(v) => out.push(v),
            Err(_) => SKIPPED.with(|s| s.set(s.get() + 1)),
        }
    }
    Some(out)
}

/// `query` for names and labels: rows it can't read are not event rows, so
/// they don't count as skipped.
fn lookup<T>(c: &Option<Connection>, sql: &str, f: impl Fn(&rusqlite::Row) -> rusqlite::Result<T>) -> Option<Vec<T>> {
    let before = SKIPPED.with(|s| s.get());
    let out = query(c, sql, f);
    SKIPPED.with(|s| s.set(before));
    out
}

fn text(r: &rusqlite::Row, i: usize) -> String {
    use rusqlite::types::ValueRef;
    match r.get_ref(i) {
        Ok(ValueRef::Text(t)) => String::from_utf8_lossy(t).into_owned(),
        Ok(ValueRef::Integer(n)) => n.to_string(),
        Ok(ValueRef::Real(f)) => f.to_string(),
        _ => String::new(),
    }
}

thread_local! {
    /// Rows `query` couldn't read during the current `load` (it runs on one thread).
    static SKIPPED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A WSDL alarm name as shown: "PLLD-0-1-FAIL-ALARM" -> "PLLD 0.1 FAIL ALARM",
/// "LiquidSensorFuelAlarm" -> "LIQUID SENSOR FUEL ALARM".
fn wsdl_alarm_label(s: &str) -> String {
    if !s.contains('-') {
        return words(s).to_uppercase();
    }
    let parts: Vec<&str> = s.split('-').collect();
    let mut out: Vec<String> = vec![];
    let mut i = 0;
    while i < parts.len() {
        // Two single digits in a row are a rate: 0-1 -> 0.1, 3-0 -> 3.0.
        let one_digit = |p: &str| p.len() == 1 && p.chars().all(|c| c.is_ascii_digit());
        if i + 1 < parts.len() && one_digit(parts[i]) && one_digit(parts[i + 1]) {
            out.push(format!("{}.{}", parts[i], parts[i + 1]));
            i += 2;
        } else {
            out.push(parts[i].to_string());
            i += 1;
        }
    }
    out.join(" ")
}

/// Control characters (line breaks, tabs) become spaces and the ends are
/// trimmed; spaces inside a value are kept. Keeps every value on one line.
pub fn clean(s: &str) -> String {
    let s: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    s.trim().to_string()
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_string() + &c.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// `<simpleType name="X">` blocks of `<enumeration value="V"/><!-- enum const = N -->`.
fn parse_wsdl_enums(w: &str) -> HashMap<String, BTreeMap<i64, String>> {
    let mut out: HashMap<String, BTreeMap<i64, String>> = HashMap::new();
    let mut current: Option<String> = None;
    for line in w.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("<simpleType name=\"") {
            current = rest.split('"').next().map(str::to_string);
        } else if l.starts_with("</simpleType") {
            current = None;
        } else if let (Some(name), Some(rest)) = (&current, l.strip_prefix("<enumeration value=\"")) {
            let value = rest.split('"').next().unwrap_or("").to_string();
            if let Some(n) = l
                .split("enum const =")
                .nth(1)
                .and_then(|s| s.trim().trim_end_matches("-->").trim().parse::<i64>().ok())
            {
                out.entry(name.clone()).or_default().insert(n, value);
            }
        }
    }
    out
}

// ------------------------------------------------------------- audit names --

/// "InterfaceTankInfo" -> "Tank Info"; "MethodSetSLDEarlyStop" -> "Set SLD Early Stop";
/// "InterfaceTlsSecurity" -> "TLS Security".
pub fn words(s: &str) -> String {
    let s = s.strip_prefix("Interface").or_else(|| s.strip_prefix("Method")).unwrap_or(s);
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    for i in 0..c.len() {
        if i > 0 && c[i].is_uppercase() {
            let prev_lower = c[i - 1].is_lowercase() || c[i - 1].is_ascii_digit();
            let next_lower = c.get(i + 1).is_some_and(|n| n.is_lowercase());
            if prev_lower || (c[i - 1].is_uppercase() && next_lower) {
                out.push(' ');
            }
        }
        out.push(c[i]);
    }
    out.split(' ')
        .map(|w| {
            ACRONYMS
                .iter()
                .find(|a| a.eq_ignore_ascii_case(w))
                .map(|a| a.to_string())
                .unwrap_or_else(|| w.to_string())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Audit `Interface`/`Method` codes = `FeatureList.FeatureID` / `SettingList.SettingID`
/// in ValidationDB (the other *ValidationDB files fill gaps).
fn audit_names(ex: &Extracted) -> HashMap<(i64, i64), (String, String)> {
    let mut map = HashMap::new();
    let Ok(dir) = fs::read_dir(&ex.dir) else { return map };
    let mut files: Vec<String> = dir
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with("ValidationDB.tls"))
        .collect();
    files.sort_by_key(|n| n != "ValidationDB.tls");
    for f in files {
        for (fi, si, iface, method) in lookup(
            &ex.db(&f),
            "select s.FeatureID, s.SettingID, f.TlsInterfaces, s.SettingName from SettingList s \
             join FeatureList f on f.FeatureID = s.FeatureID",
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, text(r, 2), text(r, 3))),
        )
        .unwrap_or_default()
        {
            map.entry((fi, si)).or_insert((iface, method));
        }
    }
    map
}

// -------------------------------------------------------------------- time --

pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, m, d)
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

pub struct Clock {
    /// Standard-time offset, seconds east of UTC.
    pub std: i64,
    /// US daylight saving (2nd Sunday of March to 1st Sunday of November, 02:00).
    pub dst: bool,
    pub name: String,
    /// The zone isn't one this program knows: a fixed offset (from status.log,
    /// or UTC), without daylight saving.
    pub fixed: bool,
}

pub fn hm(secs: i64) -> String {
    let o = secs / 60;
    format!("UTC{}{:02}:{:02}", if o < 0 { '-' } else { '+' }, o.abs() / 60, o.abs() % 60)
}

pub fn nth_sunday(y: i64, m: i64, n: i64) -> i64 {
    let first = days_from_civil(y, m, 1);
    let dow = (first + 4).rem_euclid(7); // 1970-01-01 was a Thursday; 0 = Sunday
    first + (7 - dow) % 7 + 7 * (n - 1)
}

impl Clock {
    pub fn offset_at(&self, utc: i64) -> i64 {
        if !self.dst {
            return self.std;
        }
        let (y, _, _) = civil_from_days((utc + self.std).div_euclid(86400));
        let start = nth_sunday(y, 3, 2) * 86400 + 2 * 3600 - self.std;
        let end = nth_sunday(y, 11, 1) * 86400 + 2 * 3600 - (self.std + 3600);
        if utc >= start && utc < end { self.std + 3600 } else { self.std }
    }
    /// Console local wall time (seconds) -> UTC. In the repeated fall-back hour
    /// this gives the earlier (daylight) instant.
    pub fn to_utc(&self, local: i64) -> i64 {
        let daylight = local - (self.std + if self.dst { 3600 } else { 0 });
        if self.offset_at(daylight) == self.std + 3600 {
            daylight
        } else {
            local - self.std
        }
    }
    /// Like `to_utc`, but the later (standard) instant in the repeated hour;
    /// for the end of a range, so the second pass of that hour is included.
    pub fn to_utc_latest(&self, local: i64) -> i64 {
        let standard = local - self.std;
        if self.offset_at(standard) == self.std {
            standard
        } else {
            self.to_utc(local)
        }
    }
    pub fn local(&self, utc: i64) -> String {
        let t = utc + self.offset_at(utc);
        let (y, m, d) = civil_from_days(t.div_euclid(86400));
        let s = t.rem_euclid(86400);
        format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    }
    pub fn zone(&self) -> String {
        if self.dst {
            format!("{} ({}/{})", self.name, hm(self.std), hm(self.std + 3600))
        } else {
            hm(self.std)
        }
    }
}

/// The console's zone from /etc/timezone (`TZ=":US/Mountain"`).
pub fn clock_from_timezone(s: &str) -> Option<Clock> {
    let name = s
        .trim()
        .trim_start_matches("TZ=")
        .trim_matches('"')
        .trim_start_matches(':')
        .trim()
        .to_string();
    let (std_h, dst) = match name.as_str() {
        "UTC" | "Etc/UTC" | "GMT" => (0, false),
        "US/Eastern" | "America/New_York" | "America/Detroit" | "America/Toronto" | "Canada/Eastern" => (-5, true),
        "US/Central" | "America/Chicago" | "America/Winnipeg" | "Canada/Central" => (-6, true),
        "America/Regina" | "Canada/Saskatchewan" => (-6, false),
        "US/Mountain" | "America/Denver" | "America/Boise" | "America/Edmonton" | "Canada/Mountain" => (-7, true),
        "US/Arizona" | "America/Phoenix" => (-7, false),
        "US/Pacific" | "America/Los_Angeles" | "America/Vancouver" | "Canada/Pacific" => (-8, true),
        "US/Alaska" | "America/Anchorage" => (-9, true),
        "US/Hawaii" | "Pacific/Honolulu" => (-10, false),
        _ => return None,
    };
    Some(Clock {
        std: std_h * 3600,
        dst,
        name,
        fixed: false,
    })
}

/// The last offset in status.log's `=== 2026-11-05 15:30:03 -0700 ===` lines.
pub fn offset_from_status(s: &str) -> Option<i64> {
    s.lines().rev().find_map(|l| {
        let l = l.trim().strip_prefix("===")?.strip_suffix("===")?.trim();
        let z = l.split_whitespace().nth(2)?;
        let b = z.as_bytes();
        if b.len() != 5 || !(b[0] == b'+' || b[0] == b'-') || !b[1..].iter().all(u8::is_ascii_digit) {
            return None;
        }
        let h: i64 = z.get(1..3)?.parse().ok()?;
        let m: i64 = z.get(3..5)?.parse().ok()?;
        let v = h * 3600 + m * 60;
        Some(if z.starts_with('-') { -v } else { v })
    })
}

/// "2026-11-04", "2026-11-04 12:07" or "2026-11-04 12:07:49" -> seconds of
/// console local wall time (since 1970). A missing time is the start of the
/// day (or, with `end`, its last second). None if it isn't a real date and time.
pub fn parse_local(s: &str, end: bool) -> Option<i64> {
    let s = s.trim();
    let (date, time) = s.split_once([' ', 'T']).unwrap_or((s, ""));
    let d: Vec<i64> = date.split('-').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    if d.len() != 3 || !(1970..=2200).contains(&d[0]) || !(1..=12).contains(&d[1]) || d[2] < 1 || d[2] > days_in_month(d[0], d[1]) {
        return None;
    }
    let t: Vec<i64> = if time.trim().is_empty() {
        vec![]
    } else {
        time.trim().split(':').map(|p| p.parse().ok()).collect::<Option<_>>()?
    };
    if t.len() > 3 {
        return None;
    }
    let def = if end { [23, 59, 59] } else { [0, 0, 0] };
    let h = *t.first().unwrap_or(&def[0]);
    let mi = *t.get(1).unwrap_or(&def[1]);
    let se = *t.get(2).unwrap_or(if t.len() == 2 && end { &59 } else { &def[2] });
    if !(0..=23).contains(&h) || !(0..=59).contains(&mi) || !(0..=59).contains(&se) {
        return None;
    }
    Some(days_from_civil(d[0], d[1], d[2]) * 86400 + h * 3600 + mi * 60 + se)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_input_is_refused_not_panicked_on() {
        assert_eq!(offset_from_status("=== 2026-11-05 15:30:03 +0\u{e9}0 ==="), None);
        assert_eq!(offset_from_status("=== 2026-11-05 15:30:03 +0130 ==="), Some(5400));
        assert_eq!(parse_local("2026-11-04 -3:00", false), None);
        assert_eq!(parse_local("2026-11-04 03:-1", false), None);
        assert!(safe_db_name("AlarmDB.tls"));
        for bad in ["C:evil.tls", "x:.tls", "..tls", "a b.tls", "a\\b.tls", "con<.tls"] {
            assert!(!safe_db_name(bad), "{bad}");
        }
        assert_eq!(read_capped(&mut &b"12345"[..], 5).as_deref(), Some("12345"));
        assert_eq!(read_capped(&mut &b"123456"[..], 5), None);
    }

    #[test]
    fn mountain_time_with_daylight_saving() {
        let c = clock_from_timezone("TZ=\":US/Mountain\"\n").unwrap();
        assert_eq!(c.local(1793819269), "2026-11-04 12:07:49"); // MST
        let oct = days_from_civil(2026, 10, 21) * 86400 + 18 * 3600 + 46 * 60 + 11;
        assert_eq!(c.local(oct), "2026-10-21 12:46:11"); // MDT
        assert_eq!(civil_from_days(nth_sunday(2026, 3, 2)), (2026, 3, 8));
        assert_eq!(civil_from_days(nth_sunday(2026, 11, 1)), (2026, 11, 1));
        // Round trips either side of the change.
        for (y, mo, d, h) in [(2026, 11, 5, 15), (2026, 10, 21, 12), (2026, 11, 1, 3)] {
            let local = days_from_civil(y, mo, d) * 86400 + h * 3600;
            assert_eq!(c.local(c.to_utc(local)), format!("{y:04}-{mo:02}-{d:02} {h:02}:00:00"));
        }
    }

    #[test]
    fn status_offset_uses_the_last_line() {
        assert_eq!(
            offset_from_status("=== 2026-06-09 17:40:01 +0000 ===\n=== 2026-11-05 15:30:03 -0700 ===\n"),
            Some(-25200)
        );
    }

    #[test]
    fn audit_words() {
        assert_eq!(words("InterfaceTankInfo"), "Tank Info");
        assert_eq!(words("MethodSetSLDEarlyStop"), "Set SLD Early Stop");
        assert_eq!(words("InterfaceTlsSecurity"), "TLS Security");
        assert_eq!(words("MethodSetCsldClimateFactor"), "Set CSLD Climate Factor");
        assert_eq!(words("MethodSetTcpIpStaticAddress"), "Set TCP IP Static Address");
    }

    #[test]
    fn time_box_input() {
        let day = days_from_civil(2026, 11, 4) * 86400;
        assert_eq!(parse_local("2026-11-04", false), Some(day));
        assert_eq!(parse_local("2026-11-04", true), Some(day + 86399));
        assert_eq!(parse_local("2026-11-4 9:05", false), Some(day + 9 * 3600 + 300));
        assert_eq!(parse_local("2026-11-04 9:05", true), Some(day + 9 * 3600 + 300 + 59));
        assert_eq!(parse_local("2026-11-04 12:07:49", true), Some(day + 12 * 3600 + 7 * 60 + 49));
        assert_eq!(parse_local("11/04/2026", false), None);
        assert_eq!(parse_local("2026-13-01", false), None);
        assert_eq!(parse_local("2026-02-31", false), None);
        assert_eq!(parse_local("2028-02-29", false).is_some(), true);
    }

    #[test]
    fn values_stay_on_one_line() {
        assert_eq!(clean("  a\r\nb\tc  "), "a  b c");
        assert_eq!(clean("two  spaces"), "two  spaces");
    }

    #[test]
    fn alarm_labels() {
        assert_eq!(wsdl_alarm_label("PLLD-0-1-FAIL-ALARM"), "PLLD 0.1 FAIL ALARM");
        assert_eq!(wsdl_alarm_label("PLLD-3-0-FAIL-ALARM"), "PLLD 3.0 FAIL ALARM");
        assert_eq!(wsdl_alarm_label("TNK-LEAK-ALM"), "TNK LEAK ALM");
        assert_eq!(wsdl_alarm_label("LiquidSensorFuelAlarm"), "LIQUID SENSOR FUEL ALARM");
    }

    #[test]
    fn repeated_fall_back_hour() {
        let c = clock_from_timezone("US/Mountain").unwrap();
        // 2026-11-01 01:30 happens twice: 07:30 UTC (MDT) and 08:30 UTC (MST).
        let local = days_from_civil(2026, 11, 1) * 86400 + 3600 + 1800;
        let first = days_from_civil(2026, 11, 1) * 86400 + 7 * 3600 + 1800;
        assert_eq!(c.to_utc(local), first);
        assert_eq!(c.to_utc_latest(local), first + 3600);
        // Outside that hour both agree.
        for (y, m, d) in [(2026, 11, 5), (2026, 7, 5)] {
            let noon = days_from_civil(y, m, d) * 86400 + 12 * 3600;
            assert_eq!(c.to_utc(noon), c.to_utc_latest(noon));
        }
    }

    #[test]
    fn wsdl_enums() {
        let w = "<simpleType name=\"DeviceType\">\n<enumeration value=\"TANKDEVTYPE\"/><!-- enum const = 2 -->\n</simpleType>";
        assert_eq!(parse_wsdl_enums(w)["DeviceType"][&2], "TANKDEVTYPE");
    }

    /// Loads a real snapshot: `TLS450_SNAPSHOT=<file.tar> cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn load_real_snapshot() {
        let p = std::env::var("TLS450_SNAPSHOT").expect("set TLS450_SNAPSHOT");
        let d = load(Path::new(&p)).unwrap();
        assert!(!d.rows.is_empty());
        for k in Kind::ALL {
            let of: Vec<&Row> = d.rows.iter().filter(|r| r.kind == k).collect();
            println!("== {} ({}, {} with no time)", k.name(), of.len(), d.no_time[k.index()]);
            for r in of.iter().rev().take(3) {
                println!("{}{}", r.columns().join(" | "), if r.repeat { "  [repeat]" } else { "" });
            }
        }
        println!(
            "{} rows, zone {}, missing {:?}, unreadable {:?}, skipped {}",
            d.rows.len(),
            d.clock.zone(),
            d.missing,
            d.unreadable,
            d.skipped
        );
    }
}
