//! The console's other records, one reader per table: static and CSLD leak
//! tests, line leak (PLLD) tests and their diagnostics, compliance records,
//! deliveries, inventory and shifts, power off/on, the application log,
//! auto events, ISD records and the program log files.
//!
//! Every table is read with `select *` and its columns looked up by name, so
//! a console with extra columns, or without a column a value uses, still
//! loads. A table that is not in the snapshot gives no rows (a site without
//! that feature); a table that is there but lacks a needed column is listed
//! as unreadable. Codes are named from the console's WSDL where it names them
//! (enum names noted beside each); a code it does not name is shown as a number.
//!
//! Not read, on purpose: `ManifoldedDeliveryHist` (the same deliveries as
//! `StdDeliveryHist`), `PLLDTestHistory` (the same tests as `UserResultHistory`,
//! without results), `CSLDVolumeHistory`, `HourlyPassed_3_0_Tests` and
//! `ProfileTestResult` (rolling counters with no event time), and status.log
//! (memory and disk figures every ten minutes).

use std::collections::{HashMap, HashSet};

use rusqlite::types::Value;

use super::{ACRONYMS, Ctx, Extracted, Kind, Out, Row, clean, parse_local, words};

/// One table row, by lower-case column name.
struct Rec(HashMap<String, Value>);

impl Rec {
    fn get(&self, c: &str) -> Option<&Value> {
        self.0.get(&c.to_ascii_lowercase())
    }
    fn i(&self, c: &str) -> Option<i64> {
        match self.get(c)? {
            Value::Integer(n) => Some(*n),
            Value::Real(f) if f.is_finite() => Some(*f as i64),
            Value::Text(t) => t.trim().parse().ok(),
            _ => None,
        }
    }
    fn f(&self, c: &str) -> Option<f64> {
        match self.get(c)? {
            Value::Integer(n) => Some(*n as f64),
            Value::Real(f) if f.is_finite() => Some(*f),
            Value::Text(t) => t.trim().parse().ok().filter(|f: &f64| f.is_finite()),
            _ => None,
        }
    }
    fn s(&self, c: &str) -> String {
        match self.get(c) {
            Some(Value::Text(t)) => clean(t),
            Some(Value::Integer(n)) => n.to_string(),
            Some(Value::Real(f)) => f.to_string(),
            _ => String::new(),
        }
    }
}

/// Every row of `db.table`. Empty when the file or table isn't there; when it
/// is there but can't be read, or lacks one of `need`, it is also listed in
/// `out.unreadable`.
fn table(ex: &Extracted, out: &mut Out, db: &str, name: &str, need: &[&str]) -> Vec<Rec> {
    let Some(c) = ex.db(&format!("{db}.tls")) else { return vec![] };
    let exists = c
        .query_row(
            "select count(*) from sqlite_master where type = 'table' and name = ?1",
            [name],
            |r| r.get::<_, i64>(0),
        )
        .is_ok_and(|n| n > 0);
    if !exists {
        return vec![];
    }
    let read = || -> rusqlite::Result<Option<Vec<Rec>>> {
        let mut st = c.prepare(&format!("select * from \"{name}\""))?;
        let cols: Vec<String> = st.column_names().iter().map(|s| s.to_ascii_lowercase()).collect();
        if need.iter().any(|n| !cols.contains(&n.to_ascii_lowercase())) {
            return Ok(None);
        }
        let mut rows = st.query([])?;
        let mut v = vec![];
        while let Some(r) = rows.next()? {
            let mut m = HashMap::with_capacity(cols.len());
            for (i, col) in cols.iter().enumerate() {
                m.insert(col.clone(), r.get::<_, Value>(i)?);
            }
            v.push(Rec(m));
        }
        Ok(Some(v))
    };
    match read() {
        Ok(Some(v)) => v,
        _ => {
            out.unreadable.push(format!("{db} {name}"));
            vec![]
        }
    }
}

// ---------------------------------------------------------------- wording --

/// 2820.7 -> "2,821 gal".
fn gal(v: f64) -> String {
    format!("{} gal", thousands(v.round() as i64))
}

fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// Volume, fuel height and temperature, the ones present.
fn level(r: &Rec, vol: &str, height: &str, temp: &str) -> String {
    let mut parts = vec![];
    if let Some(v) = r.f(vol) {
        parts.push(gal(v));
    }
    if let Some(h) = r.f(height) {
        parts.push(format!("{h:.2} in"));
    }
    if let Some(t) = r.f(temp) {
        parts.push(format!("{t:.1} °F"));
    }
    parts.join(", ")
}

/// 7770 -> "2 h 9 min"; 45 -> "45 s".
fn duration(secs: i64) -> String {
    let s = secs.max(0);
    match (s / 3600, s / 60 % 60) {
        (0, 0) => format!("{s} s"),
        (0, m) => format!("{m} min"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// A WSDL name as lower-case words, acronyms kept: "RateTestPass" -> "rate test pass".
fn phrase(s: &str) -> String {
    words(s)
        .split(' ')
        .map(|w| if ACRONYMS.contains(&w) { w.to_string() } else { w.to_lowercase() })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A WSDL enumeration value as words, or "<what> <n>" when the WSDL has no name for it.
fn named(ctx: &Ctx, ty: &str, v: Option<i64>, what: &str) -> String {
    match v {
        Some(n) => ctx.enum_name(ty, n).map(phrase).unwrap_or(format!("{what} {n}")),
        None => String::new(),
    }
}

/// PLLD `TestType` (ResultHistory, UserResultHistory, LastPassedTest, AutoConfirmHistory).
fn line_rate(t: Option<i64>) -> String {
    match t {
        Some(1) => "3.0 gph".into(),
        Some(2) => "0.2 gph".into(),
        Some(3) => "0.1 gph".into(),
        Some(n) => format!("type {n}"),
        None => "unknown".into(),
    }
}

/// WSDL `PLLDDiagnosticResult`.
fn line_result(r: Option<i64>) -> String {
    match r {
        Some(0) => "invalid".into(),
        Some(1) => "passed".into(),
        Some(2) => "failed".into(),
        Some(3) => "error".into(),
        Some(4) => "high pressure".into(),
        Some(n) => format!("result {n}"),
        None => "no result".into(),
    }
}

/// WSDL `LeakTestResult`.
fn tank_result(r: i64) -> String {
    match r {
        0 => "invalid".into(),
        1 => "passed".into(),
        2 => "failed".into(),
        3 => "error".into(),
        4 => "no result".into(),
        n => format!("result {n}"),
    }
}

/// WSDL `TypeOfComplianceTest`: (test, is a line test).
fn compliance_test(t: Option<i64>) -> (String, bool) {
    match t {
        Some(0) => ("0.1 gph static leak test".into(), false),
        Some(1) => ("0.2 gph static leak test".into(), false),
        Some(2) => ("gross static leak test".into(), false),
        Some(3) => ("0.2 gph CSLD test".into(), false),
        Some(4) => ("0.1 gph line leak test".into(), true),
        Some(5) => ("0.2 gph line leak test".into(), true),
        Some(6) => ("3.0 gph line leak test".into(), true),
        Some(n) => (format!("test type {n}"), false),
        None => ("unknown test".into(), false),
    }
}

/// A number with up to `dp` decimals, or "?" when the column is missing.
fn num(v: Option<f64>, dp: usize) -> String {
    v.map(|v| format!("{v:.dp$}")).unwrap_or("?".into())
}

// ---------------------------------------------------------------- readers --

/// Static (SLD) leak tests: a row when each starts and one when it ends, with
/// its result. False when the table isn't in the snapshot.
pub(super) fn static_tests(ex: &Extracted, ctx: &Ctx, out: &mut Out) -> bool {
    let present = ex
        .db("SLDStatusDB.tls")
        .and_then(|c| {
            c.query_row(
                "select count(*) from sqlite_master where type = 'table' and name = 'SLDHistory'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .ok()
        })
        .is_some_and(|n| n > 0);
    for r in table(
        ex,
        out,
        "SLDStatusDB",
        "SLDHistory",
        &["AnalyzerID", "StartTime", "EndTime", "StartMethod"],
    ) {
        let method = r.i("StartMethod");
        let (kind, who) = match method {
            Some(1) => (Kind::ManualTest, "Manual".to_string()),
            Some(5) => (Kind::AutoTest, "Automatic".to_string()),
            Some(n) => (Kind::AutoTest, format!("Method {n}")),
            None => (Kind::AutoTest, "Unknown".to_string()),
        };
        // WSDL SLDLeakTestType; LeakTestResult_1.._3 are the 0.2, 0.1 and gross results.
        let test_type = r.i("TestType");
        let (rate, result) = match test_type {
            Some(0) => ("0.2 gph test", r.i("LeakTestResult_1")),
            Some(1) => ("0.1 gph test", r.i("LeakTestResult_2")),
            Some(2) => ("gross test", r.i("LeakTestResult_3")),
            _ => (
                "test",
                ["LeakTestResult_1", "LeakTestResult_2", "LeakTestResult_3"]
                    .iter()
                    .filter_map(|c| r.i(c))
                    .find(|&v| v != 0)
                    .or(Some(0)),
            ),
        };
        let device = ctx.tank(r.i("AnalyzerID").unwrap_or(0));
        let mut started = rate.to_string();
        if let Some(v) = r.f("StartVolume") {
            started.push_str(&format!(", start volume {}", gal(v)));
        }
        let mut ended = result.map(tank_result).unwrap_or("no result".into());
        if let Some(m) = r.i("DurationInMinutes") {
            ended.push_str(&format!(", duration {m} min"));
        }
        for (at, what, value) in [(r.i("StartTime"), "started", started), (r.i("EndTime"), "ended", ended)] {
            out.push(
                ctx,
                at,
                Row {
                    kind,
                    device: device.clone(),
                    event: format!("{who} static leak test {what}"),
                    value,
                    ..Row::default()
                },
            );
        }
    }
    present
}

/// Everything after the audit trail, alarms and static tests.
pub(super) fn read_all(ex: &Extracted, ctx: &Ctx, out: &mut Out) {
    log_management(ex, ctx, out);
    alarm_period(ex, ctx, out);
    csld(ex, ctx, out);
    line_tests(ex, ctx, out);
    compliance(ex, ctx, out);
    deliveries(ex, ctx, out);
    inventory(ex, ctx, out);
    power(ex, ctx, out);
    app_log(ex, ctx, out);
    auto_events(ex, ctx, out);
    isd(ex, ctx, out);
    program_logs(ex, ctx, out);
}

/// The Log Management report: the audit trail's layout and codes. An entry
/// that matches an audit row exactly is marked `repeat` (one per audit row).
fn log_management(ex: &Extracted, ctx: &Ctx, out: &mut Out) {
    type Key = (i64, String, String, String, String, String);
    let key = |r: &Row| -> Key {
        (
            r.utc,
            r.device.clone(),
            r.event.clone(),
            r.user.clone(),
            r.value.clone(),
            r.previous.clone(),
        )
    };
    let mut audit: HashMap<Key, usize> = HashMap::new();
    for r in out.rows.iter().filter(|r| r.kind == Kind::Audit) {
        *audit.entry(key(r)).or_default() += 1;
    }
    let need = ["TimeStamp", "Interface", "Method"];
    for r in table(ex, out, "LogManagementHistoryDB", "LogManagementHistory", &need) {
        let (Some(i), Some(m)) = (r.i("Interface"), r.i("Method")) else {
            out.skipped += 1;
            continue;
        };
        let (device, event) = ctx.audit_event(i, m, r.i("ID").unwrap_or(-1));
        let n = out.rows.len();
        out.push(
            ctx,
            r.i("TimeStamp"),
            Row {
                kind: Kind::LogManagement,
                device,
                user: r.s("User"),
                event,
                value: r.s("NewValue"),
                previous: r.s("OldValue"),
                ..Row::default()
            },
        );
        if out.rows.len() > n {
            let k = key(&out.rows[n]);
            if let Some(c) = audit.get_mut(&k).filter(|c| **c > 0) {
                *c -= 1;
                out.rows[n].repeat = true;
            }
        }
    }
}

/// The alarm period report's records: a device's alarm status (WSDL DeviceAlarmStatus).
fn alarm_period(ex: &Extracted, ctx: &Ctx, out: &mut Out) {
    let need = ["LastChangeDateTime", "DeviceType", "DeviceID"];
    for r in table(ex, out, "AlarmDB", "AlarmPeriodNormalHistory", &need) {
        let (Some(t), Some(d)) = (r.i("DeviceType"), r.i("DeviceID")) else {
            out.skipped += 1;
            continue;
        };
        out.push(
            ctx,
            r.i("LastChangeDateTime"),
            Row {
                kind: Kind::AlarmPeriod,
                device: ctx.device(t, d),
                event: "Alarm period report record".into(),
                value: named(ctx, "DeviceAlarmStatus", r.i("DeviceAlarmStatus"), "status"),
                ..Row::default()
            },
        );
    }
}

/// CSLD: rate tests (at their end), result changes (WSDL CsldTestResult) and
/// each tank's current test status.
fn csld(ex: &Extracted, ctx: &Ctx, out: &mut Out) {
    for r in table(ex, out, "CSLDStatusDB", "CSLDRateRecordHistory", &["AnalyzerID", "StartTime"]) {
        let dur = r.i("TestDuration").unwrap_or(0);
        let mut value = match r.f("LeakRate") {
            Some(v) => format!("leak rate {v:+.4} gph"),
            None => "leak rate ?".into(),
        };
        value.push_str(&format!(" over {}", duration(dur)));
        out.push(
            ctx,
            r.i("StartTime").map(|s| s + dur),
            Row {
                kind: Kind::Csld,
                device: ctx.tank(r.i("AnalyzerID").unwrap_or(0)),
                event: "CSLD rate test".into(),
                value,
                ..Row::default()
            },
        );
    }
    for r in table(ex, out, "CSLDStatusDB", "CSLDStateChangesHistory", &["tod", "AnalyzerID"]) {
        out.push(
            ctx,
            r.i("tod"),
            Row {
                kind: Kind::Csld,
                device: ctx.tank(r.i("AnalyzerID").unwrap_or(0)),
                event: "CSLD result changed".into(),
                value: named(ctx, "CsldTestResult", r.i("result"), "result"),
                ..Row::default()
            },
        );
    }
    for r in table(ex, out, "CSLDStatusDB", "CSLDPosRejectStatusChangesHistory", &["tod", "AnalyzerID"]) {
        out.push(
            ctx,
            r.i("tod"),
            Row {
                kind: Kind::Csld,
                device: ctx.tank(r.i("AnalyzerID").unwrap_or(0)),
                event: "CSLD positive reject status changed".into(),
                value: r.i("status").map(|s| format!("status {s}")).unwrap_or_default(),
                ..Row::default()
            },
        );
    }
    for r in table(ex, out, "CSLDStatusDB", "CSLDTestStatus", &["time", "AnalyzerID"]) {
        // One slot per possible tank; a tank without CSLD has time 0.
        if r.i("time") == Some(0) {
            continue;
        }
        out.push(
            ctx,
            r.i("time"),
            Row {
                kind: Kind::Csld,
                device: ctx.tank(r.i("AnalyzerID").unwrap_or(0)),
                event: "CSLD test status (latest)".into(),
                value: named(ctx, "CsldTestResult", r.i("rate_test_result"), "result"),
                ..Row::default()
            },
        );
    }
}

/// Line leak (PLLD) tests: the results the console reports, and the details
/// behind them. A line's AnalyzerID is its line number.
fn line_tests(ex: &Extracted, ctx: &Ctx, out: &mut Out) {
    const DB: &str = "PLLDInternalsHistoriesDB";
    let line = |r: &Rec| ctx.line(r.i("AnalyzerID").unwrap_or(0));
    for r in table(ex, out, DB, "UserResultHistory", &["TestTime", "AnalyzerID"]) {
        out.push(
            ctx,
            r.i("TestTime"),
            Row {
                kind: Kind::LineTest,
                device: line(&r),
                event: format!("{} line leak test {}", line_rate(r.i("TestType")), line_result(r.i("Result"))),
                ..Row::default()
            },
        );
    }
    for r in table(ex, out, DB, "ResultHistory", &["TestTime", "AnalyzerID"]) {
        let mut v = vec![line_result(r.i("Result"))];
        if let Some(f) = r.f("FailRatio") {
            v.push(format!("fail ratio {f:.3}"));
        }
        if let Some(m) = r.i("MinsTestLength") {
            v.push(format!("{m} min"));
        }
        if let Some(p) = r.f("A_p1") {
            v.push(format!("start pressure {p:.1} psi"));
        }
        out.push(
            ctx,
            r.i("TestTime"),
            Row {
                kind: Kind::LineDiag,
                device: line(&r),
                event: format!("{} line test details", line_rate(r.i("TestType"))),
                value: v.join(", "),
                ..Row::default()
            },
        );
    }
    for r in table(ex, out, DB, "PrecisionDiagHistory", &["Time", "AnalyzerID"]) {
        out.push(
            ctx,
            r.i("Time"),
            Row {
                kind: Kind::LineDiag,
                device: line(&r),
                event: format!(
                    "{} precision test step{}",
                    line_rate(r.i("TestType")),
                    r.i("DiagType").map(|d| format!(" {d}")).unwrap_or_default()
                ),
                value: format!(
                    "pressure {} psi, ramp rate {}, fail ratio {}",
                    num(r.f("Pressure"), 1),
                    num(r.f("RampRate"), 4),
                    num(r.f("FailRatio"), 3)
                ),
                ..Row::default()
            },
        );
    }
    for r in table(ex, out, DB, "PortDiagnosticHistory", &["time", "AnalyzerID"]) {
        // WSDL PLLDDiagnosticTestType.
        let test = match r.i("DiagType") {
            Some(0) => "gross".to_string(),
            Some(1) => "0.2 gph".into(),
            Some(2) => "0.1 gph".into(),
            Some(3) => "3.0 gph".into(),
            Some(4) => "mid-range".into(),
            Some(5) => "no-vent abort".into(),
            Some(n) => format!("type {n}"),
            None => "unknown".into(),
        };
        out.push(
            ctx,
            r.i("time"),
            Row {
                kind: Kind::LineDiag,
                device: line(&r),
                event: format!("Line pressure check ({test})"),
                value: format!(
                    "pump on {} psi, rate {} (limit {})",
                    num(r.f("PumpOnPressure"), 1),
                    num(r.f("CalculateRate"), 5),
                    num(r.f("FailRateThreshold"), 5)
                ),
                ..Row::default()
            },
        );
    }
    for r in table(ex, out, DB, "NoSpikeDiagnosticHistory", &["time", "AnalyzerID"]) {
        out.push(
            ctx,
            r.i("time"),
            Row {
                kind: Kind::LineDiag,
                device: line(&r),
                event: "Line no-spike check".into(),
                value: r.s("NoSpikeValue"),
                ..Row::default()
            },
        );
    }
    for r in table(ex, out, DB, "AutoConfirmHistory", &["PeriodEndTime", "AnalyzerID"]) {
        out.push(
            ctx,
            r.i("PeriodEndTime"),
            Row {
                kind: Kind::LineDiag,
                device: line(&r),
                event: format!("{} auto-confirm period ended", line_rate(r.i("TestType"))),
                value: format!(
                    "{}, {} passed, {} failed",
                    line_result(r.i("Result")),
                    r.i("PassCnt").map(|n| n.to_string()).unwrap_or("?".into()),
                    r.i("FailCnt").map(|n| n.to_string()).unwrap_or("?".into())
                ),
                ..Row::default()
            },
        );
    }
}

/// Compliance records (WSDL TypeOfComplianceTest) and each line's last passed
/// test (PLLD's own record of it; one that repeats a compliance row is left out).
fn compliance(ex: &Extracted, ctx: &Ctx, out: &mut Out) {
    let start = out.rows.len();
    const DB: &str = "ComplianceHistoryDB";
    for (tbl, what) in [
        ("MonthlyComplianceHistory", "Monthly compliance"),
        ("LastPassedComplianceHistory", "Last passed"),
    ] {
        for r in table(ex, out, DB, tbl, &["TestDateTime", "DeviceID"]) {
            let (test, is_line) = compliance_test(r.i("TestType"));
            let id = r.i("DeviceID").unwrap_or(0);
            let mut v = vec![];
            if let Some(h) = r.f("TotalHours").filter(|h| *h > 0.0) {
                v.push(format!("{h:.1} h"));
            }
            if let Some(a) = r.f("AverageVolume").filter(|a| *a > 0.0) {
                v.push(format!("average {}", gal(a)));
            }
            out.push(
                ctx,
                r.i("TestDateTime"),
                Row {
                    kind: Kind::Compliance,
                    device: if is_line { ctx.line(id) } else { ctx.tank(id) },
                    event: format!("{what}: {test}"),
                    value: v.join(", "),
                    ..Row::default()
                },
            );
        }
    }
    let seen: HashSet<(i64, String, String)> = out.rows[start..]
        .iter()
        .map(|r| (r.utc, r.device.clone(), r.event.clone()))
        .collect();
    for r in table(ex, out, DB, "TidelLeakResult", &["TankID", "StartTime"]) {
        // A fixed set of slots; an unused one is all zeros.
        if r.i("StartTime") == Some(0) {
            continue;
        }
        out.push(
            ctx,
            r.i("StartTime"),
            Row {
                kind: Kind::Compliance,
                device: ctx.tank(r.i("TankID").unwrap_or(0)),
                event: "Tidel leak test".into(),
                value: format!("status {}, rate {}", r.s("TestStatus"), num(r.f("LeakTestRate"), 4)),
                ..Row::default()
            },
        );
    }
    for r in table(ex, out, "PLLDInternalsRecordsDB", "LastPassedTest", &["TestTime", "AnalyzerID"]) {
        let device = ctx.line(r.i("AnalyzerID").unwrap_or(0));
        let event = format!("Last passed: {} line leak test", line_rate(r.i("TestType")));
        if r.i("TestTime").is_some_and(|t| seen.contains(&(t, device.clone(), event.clone()))) {
            continue;
        }
        out.push(
            ctx,
            r.i("TestTime"),
            Row {
                kind: Kind::Compliance,
                device,
                event,
                ..Row::default()
            },
        );
    }
}

/// Deliveries: a row at the start and one at the end (Value the end level,
/// Previous the start level).
fn deliveries(ex: &Extracted, ctx: &Ctx, out: &mut Out) {
    let need = ["TankNumber", "StartDateTime", "EndDateTime"];
    for r in table(ex, out, "DeliveryDB", "StdDeliveryHist", &need) {
        let tank = ctx.tank(r.i("TankNumber").unwrap_or(0));
        let start = level(&r, "StartVolume", "StartHeight", "StartTemperature");
        let end = level(&r, "EndVolume", "EndHeight", "EndTemperature");
        let amount = match (r.f("StartVolume"), r.f("EndVolume")) {
            (Some(a), Some(b)) => format!(" ({} delivered)", gal(b - a)),
            _ => String::new(),
        };
        out.push(
            ctx,
            r.i("StartDateTime"),
            Row {
                kind: Kind::Delivery,
                device: tank.clone(),
                event: "Delivery started".into(),
                value: start.clone(),
                ..Row::default()
            },
        );
        out.push(
            ctx,
            r.i("EndDateTime"),
            Row {
                kind: Kind::Delivery,
                device: tank,
                event: format!("Delivery ended{amount}"),
                value: end,
                previous: start,
                ..Row::default()
            },
        );
    }
    for r in table(ex, out, "DeliveryDB", "PowerOutageDeliveryHistory", &["TankId", "EndTimeStamp"]) {
        let amount = match (r.f("StartFuelVolume"), r.f("EndFuelVolume")) {
            (Some(a), Some(b)) => format!(" ({})", gal(b - a)),
            _ => String::new(),
        };
        out.push(
            ctx,
            r.i("EndTimeStamp"),
            Row {
                kind: Kind::Delivery,
                device: ctx.tank(r.i("TankId").unwrap_or(0)),
                event: format!("Delivery during a power outage{amount}"),
                value: level(&r, "EndFuelVolume", "EndFuelHeight", "-"),
                previous: level(&r, "StartFuelVolume", "StartFuelHeight", "-"),
                ..Row::default()
            },
        );
    }
    for r in table(ex, out, "DeliveryDB", "TidelDeliveryClose", &["TankNumber", "EndDateTime"]) {
        // A fixed set of slots; an unused one is all zeros.
        if r.i("EndDateTime") == Some(0) {
            continue;
        }
        out.push(
            ctx,
            r.i("EndDateTime"),
            Row {
                kind: Kind::Delivery,
                device: ctx.tank(r.i("TankNumber").unwrap_or(0)),
                event: "Tidel delivery closed".into(),
                value: r.f("DeliveryVolume").map(gal).unwrap_or_default(),
                ..Row::default()
            },
        );
    }
}

/// Inventory readings and shift closes (in either of two files; a row in
/// both is shown once), power outage inventory and generator runs.
fn inventory(ex: &Extracted, ctx: &Ctx, out: &mut Out) {
    let mut seen: HashSet<(&str, i64, i64, i64)> = HashSet::new();
    for db in ["DeliveryHistoryDB", "InventoryHistoryDB"] {
        for r in table(ex, out, db, "InventoryHistory", &["TankID", "TimeStamp"]) {
            let (tank, at, ty) = (r.i("TankID").unwrap_or(0), r.i("TimeStamp"), r.i("EntryType"));
            if !seen.insert(("inv", tank, at.unwrap_or(-1), ty.unwrap_or(-1))) {
                continue;
            }
            let event = match ty {
                Some(0) => "Inventory reading".to_string(),
                Some(1) => "Inventory at delivery start".into(),
                Some(2) => "Inventory at delivery end".into(),
                Some(n) => format!("Inventory record (type {n})"),
                None => "Inventory record".into(),
            };
            let mut value = level(&r, "FuelVolume", "FuelHeight", "AvgFuelTemperature");
            if let Some(w) = r.f("WaterHeight") {
                value.push_str(&format!(", water {w:.2} in"));
            }
            out.push(
                ctx,
                at,
                Row {
                    kind: Kind::Inventory,
                    device: ctx.tank(tank),
                    event,
                    value,
                    ..Row::default()
                },
            );
        }
        for r in table(ex, out, db, "ShiftInvHistory", &["TankID", "RequestCloseTime"]) {
            let (tank, at, shift) = (r.i("TankID").unwrap_or(0), r.i("RequestCloseTime"), r.i("ShiftID"));
            if !seen.insert(("shift", tank, at.unwrap_or(-1), shift.unwrap_or(-1))) {
                continue;
            }
            let mut value = level(&r, "FuelVolume", "FuelHeight", "AvgFuelTemperature");
            if let Some(d) = r.f("DeliveryVolume").filter(|d| *d != 0.0) {
                value.push_str(&format!(", delivered {}", gal(d)));
            }
            out.push(
                ctx,
                at,
                Row {
                    kind: Kind::Inventory,
                    device: ctx.tank(tank),
                    event: match shift {
                        Some(s) => format!("Shift {s} closed"),
                        None => "Shift closed".into(),
                    },
                    value,
                    ..Row::default()
                },
            );
        }
    }
    for r in table(
        ex,
        out,
        "InventoryHistoryDB",
        "PowerOutageHistory",
        &["TankId", "RestoredTimeStamp"],
    ) {
        out.push(
            ctx,
            r.i("RestoredTimeStamp"),
            Row {
                kind: Kind::Inventory,
                device: ctx.tank(r.i("TankId").unwrap_or(0)),
                event: "Inventory across a power outage".into(),
                value: r
                    .f("RestoredFuelVolume")
                    .map(|v| format!("{} at power on", gal(v)))
                    .unwrap_or_default(),
                previous: r
                    .f("RemovedFuelVolume")
                    .map(|v| format!("{} at power off", gal(v)))
                    .unwrap_or_default(),
                ..Row::default()
            },
        );
    }
    for r in table(ex, out, "InventoryHistoryDB", "GeneratorInventoryReport", &["TankID", "EndTime"]) {
        let mut value = format!("used {}", r.f("ConsumeVolume").map(gal).unwrap_or("?".into()));
        if let Some(d) = r.i("DurationSecs") {
            value.push_str(&format!(" over {}", duration(d)));
        }
        out.push(
            ctx,
            r.i("EndTime"),
            Row {
                kind: Kind::Inventory,
                device: ctx.tank(r.i("TankID").unwrap_or(0)),
                event: "Generator run ended".into(),
                value,
                ..Row::default()
            },
        );
    }
}

/// Power off and on, from the power reset history.
fn power(ex: &Extracted, ctx: &Ctx, out: &mut Out) {
    let need = ["PowerUpTimeStamp", "PowerDownTimeStamp"];
    for r in table(ex, out, "ApplicationLogDB", "PowerResetHistory", &need) {
        let (down, up) = (r.i("PowerDownTimeStamp"), r.i("PowerUpTimeStamp"));
        let mut on = match (down, up) {
            (Some(d), Some(u)) if d > 0 && u >= d => format!("off for {}", duration(u - d)),
            _ => String::new(),
        };
        let note = [
            r.i("ResetCode").map(|c| format!("reset code {c}")),
            Some(r.s("Note")).filter(|n| !n.is_empty()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
        if !note.is_empty() {
            on = if on.is_empty() { note } else { format!("{on} ({note})") };
        }
        for (at, event, value) in [(down, "Power off", String::new()), (up, "Power on", on)] {
            out.push(
                ctx,
                at,
                Row {
                    kind: Kind::Power,
                    device: "Console".into(),
                    event: event.into(),
                    value,
                    ..Row::default()
                },
            );
        }
    }
}

/// The application log. Its FeatureID is not the audit trail's feature
/// number (CSLD messages carry 14, the audit trail's PLLD Info), so it is
/// shown as a number rather than named.
fn app_log(ex: &Extracted, ctx: &Ctx, out: &mut Out) {
    for r in table(ex, out, "ApplicationLogDB", "ApplicationLogHistory", &["TimeStamp", "MessageText"]) {
        let value = [
            r.i("FeatureID").map(|f| format!("feature {f}")),
            r.i("MessageLogLevel").map(|l| format!("level {l}")),
            r.i("MessageCode").filter(|c| *c != 0).map(|c| format!("code {c}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
        out.push(
            ctx,
            r.i("TimeStamp"),
            Row {
                kind: Kind::AppLog,
                device: "Console".into(),
                event: r.s("MessageText"),
                value,
                ..Row::default()
            },
        );
    }
}

/// WSDL AutoEventStatus.
const AE_STATUS: [&str; 7] = [
    "any",
    "success",
    "pending",
    "failed",
    "auto event deleted",
    "contact deleted",
    "none",
];
/// WSDL AutoEventExtStatus.
const AE_EXT: [&str; 20] = [
    "none",
    "success",
    "printer not found",
    "printer not responding",
    "printer out of paper",
    "printer error",
    "device not found",
    "shutdown signal sent",
    "port busy",
    "call busy",
    "no answer",
    "no carrier",
    "no dial tone",
    "internal error",
    "connection waiting",
    "connection dropped",
    "connection idle time expired",
    "connection closed on command",
    "connection in progress",
    "shutdown signal cleared",
];

/// Automatic reports and dial-outs (the console's task log).
fn auto_events(ex: &Extracted, ctx: &Ctx, out: &mut Out) {
    for r in table(ex, out, "AutoEventsDB", "AutoEventsLog", &["EventTime"]) {
        let device = match (r.i("DeviceType"), r.i("DeviceId")) {
            (Some(t), Some(d)) if t > 0 => ctx.device(t, d),
            _ => String::new(),
        };
        let mut event = match r.i("AutoEventID") {
            Some(id) => format!("Auto event {id}"),
            None => "Auto event".into(),
        };
        let contact = r.s("ContactName");
        if !contact.is_empty() {
            event.push_str(&format!(" to {contact}"));
        }
        let pick = |list: &[&str], v: Option<i64>| -> Option<String> {
            let v = v?;
            Some(
                usize::try_from(v)
                    .ok()
                    .and_then(|i| list.get(i))
                    .map(|s| s.to_string())
                    .unwrap_or(format!("status {v}")),
            )
        };
        let mut value = pick(&AE_STATUS, r.i("Status")).unwrap_or_default();
        if let Some(e) = pick(&AE_EXT, r.i("ExtStatus")).filter(|e| e != "none" && e != "success") {
            value.push_str(&format!(" ({e})"));
        }
        if let Some(a) = r.i("Attempts") {
            value.push_str(&format!(", {a} attempt{}", if a == 1 { "" } else { "s" }));
        }
        out.push(
            ctx,
            r.i("EventTime"),
            Row {
                kind: Kind::AutoEvent,
                device,
                event,
                value: value.trim_start_matches(", ").to_string(),
                ..Row::default()
            },
        );
    }
}

/// In-station diagnostics history.
fn isd(ex: &Extracted, ctx: &Ctx, out: &mut Out) {
    for r in table(ex, out, "ISDQListDB", "ISD_pvcrack_history", &["TimeStamp"]) {
        out.push(
            ctx,
            r.i("TimeStamp"),
            Row {
                kind: Kind::Isd,
                device: r.i("tnkSensIndx").map(|i| format!("ISD sensor {i}")).unwrap_or("ISD".into()),
                event: "ISD vapor valve crack record".into(),
                value: r.s("pos_pvcrack"),
                ..Row::default()
            },
        );
    }
}

// ----------------------------------------------------------- program logs --

/// Longest event text kept for one log line (with its continuation lines).
const MAX_LOG_EVENT: usize = 400;

/// "commandprocessor-err.log-20261020", "watchdog.log.01", "x.log.2.gz" ->
/// the file the line was written to ("commandprocessor-err.log").
fn log_file_name(n: &str) -> String {
    let n = n.trim_end_matches(".gz");
    match n.find(".log") {
        Some(i) => n[..i + 4].to_string(),
        None => n.to_string(),
    }
}

/// A program log line's time (console local wall seconds) and the rest.
/// Times are `YYYY-MM-DD HH:MM:SS` or `YYYY/MM/DD HH:MM:SS`, optionally with
/// `.mmm`; the g3log files then have a tick count.
fn split_log_line(l: &str) -> Option<(i64, &str)> {
    let head = l.get(..19)?;
    let b = head.as_bytes();
    if !(b[4] == b[7] && (b[4] == b'-' || b[4] == b'/') && b[10] == b' ') {
        return None;
    }
    let local = parse_local(&head.replace('/', "-"), false)?;
    let mut rest = &l[19..];
    if let Some(r) = rest.strip_prefix('.') {
        rest = r.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let rest = rest.trim_start();
    // g3log: "167525\t[...]": the tick count before a tab.
    let rest = match rest.split_once('\t') {
        Some((ticks, after)) if !ticks.is_empty() && ticks.chars().all(|c| c.is_ascii_digit()) => after,
        _ => rest,
    };
    Some((local, rest))
}

/// "[core][ERROR] [x.cpp->f:12]\tmessage" -> ("message", "core ERROR");
/// "[AlwaysPresent/2] Starting" -> ("Starting", "AlwaysPresent/2");
/// "- DL- IniFile..." -> ("DL- IniFile...", "").
fn split_tags(rest: &str) -> (String, String) {
    let mut tags = vec![];
    let mut s = rest.trim_start();
    while let Some(r) = s.strip_prefix('[') {
        let Some(end) = r.find(']') else { break };
        let tag = &r[..end];
        if !tag.contains("->") {
            tags.push(tag.to_string());
        }
        s = r[end + 1..].trim_start();
    }
    (clean(s.trim_start_matches("- ")), tags.join(" "))
}

/// Each dated line of each program log file is a row; an undated line
/// continues the row above it (from the same file). Times are console local.
fn program_logs(ex: &Extracted, ctx: &Ctx, out: &mut Out) {
    for (name, text) in &ex.logs {
        let device = log_file_name(name);
        let mut last: Option<usize> = None;
        for line in text.lines() {
            match split_log_line(line) {
                Some((local, rest)) => {
                    let (event, value) = split_tags(rest);
                    let n = out.rows.len();
                    out.push(
                        ctx,
                        Some(ctx.clock.to_utc(local)),
                        Row {
                            kind: Kind::ProgramLog,
                            device: device.clone(),
                            event,
                            value,
                            ..Row::default()
                        },
                    );
                    last = (out.rows.len() > n).then_some(n);
                }
                None => {
                    let more = clean(line);
                    if more.is_empty() || more.chars().all(|c| c == '-' || c == '=') {
                        continue;
                    }
                    if let Some(i) = last {
                        let e = &mut out.rows[i].event;
                        if e.len() < MAX_LOG_EVENT {
                            if !e.is_empty() {
                                e.push_str(" | ");
                            }
                            e.push_str(&more);
                            if e.len() > MAX_LOG_EVENT {
                                let cut = (0..=MAX_LOG_EVENT).rev().find(|&i| e.is_char_boundary(i)).unwrap_or(0);
                                e.truncate(cut);
                                e.push_str("...");
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_lines_split() {
        let (t, rest) =
            split_log_line("2026/10/13 13:18:35 230994\t[FRAMEWORK][ERROR] [Service.cpp->post_message:128]\tTried again").unwrap();
        assert_eq!(t, parse_local("2026-10-13 13:18:35", false).unwrap());
        assert_eq!(split_tags(rest), ("Tried again".to_string(), "FRAMEWORK ERROR".to_string()));
        let (_, rest) = split_log_line("2026-06-09 22:48:02.528 [SystemLogLevel/2] CommServices Started").unwrap();
        assert_eq!(
            split_tags(rest),
            ("CommServices Started".to_string(), "SystemLogLevel/2".to_string())
        );
        let (_, rest) = split_log_line("2026-10-20 14:02:23 - DL- IniFile::setLogFlag: - logging = 1").unwrap();
        assert_eq!(split_tags(rest).0, "DL- IniFile::setLogFlag: - logging = 1");
        assert!(split_log_line("Orb Activated as <IOR:0100>").is_none());
        assert!(split_log_line("2026-13-45 25:00:00 bad").is_none());
        assert!(split_log_line("short").is_none());
        assert!(split_log_line("ééééééééééééééééééééé").is_none());
    }

    #[test]
    fn log_file_names() {
        assert_eq!(log_file_name("commandprocessor-err.log-20261020"), "commandprocessor-err.log");
        assert_eq!(log_file_name("chassis_cdm.log.3.gz"), "chassis_cdm.log");
        assert_eq!(log_file_name("watchdog.log.01"), "watchdog.log");
        assert_eq!(log_file_name("odd"), "odd");
    }

    #[test]
    fn wording() {
        assert_eq!(gal(2820.7), "2,821 gal");
        assert_eq!(gal(-1234567.0), "-1,234,567 gal");
        assert_eq!(gal(12.0), "12 gal");
        assert_eq!(duration(7770), "2 h 9 min");
        assert_eq!(duration(45), "45 s");
        assert_eq!(duration(600), "10 min");
        assert_eq!(phrase("RateTestPass"), "rate test pass");
        assert_eq!(phrase("StatusNoIdleData"), "status no idle data");
    }
}
