//! The kinds of event the program reads from a snapshot. Each kind has its own
//! Show tick box, so a user picks exactly which logs they see.

/// One kind of event. The order here is the order of the Show list.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Default)]
pub enum Kind {
    #[default]
    Audit,
    LogManagement,
    Alarm,
    AlarmPeriod,
    ManualTest,
    AutoTest,
    Csld,
    LineTest,
    LineDiag,
    Compliance,
    Delivery,
    Inventory,
    Power,
    AppLog,
    AutoEvent,
    ProgramLog,
    Isd,
}

/// The groups the Show list is split into.
pub const GROUPS: &[(&str, &[Kind])] = &[
    ("People", &[Kind::Audit, Kind::LogManagement]),
    ("Alarms", &[Kind::Alarm, Kind::AlarmPeriod]),
    (
        "Leak tests",
        &[
            Kind::ManualTest,
            Kind::AutoTest,
            Kind::Csld,
            Kind::LineTest,
            Kind::LineDiag,
            Kind::Compliance,
        ],
    ),
    ("Fuel", &[Kind::Delivery, Kind::Inventory]),
    (
        "Console",
        &[Kind::Power, Kind::AppLog, Kind::AutoEvent, Kind::ProgramLog, Kind::Isd],
    ),
];

impl Kind {
    pub const COUNT: usize = 17;
    pub const ALL: [Kind; Kind::COUNT] = [
        Kind::Audit,
        Kind::LogManagement,
        Kind::Alarm,
        Kind::AlarmPeriod,
        Kind::ManualTest,
        Kind::AutoTest,
        Kind::Csld,
        Kind::LineTest,
        Kind::LineDiag,
        Kind::Compliance,
        Kind::Delivery,
        Kind::Inventory,
        Kind::Power,
        Kind::AppLog,
        Kind::AutoEvent,
        Kind::ProgramLog,
        Kind::Isd,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// The Show tick box's text.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Audit => "Audit trail",
            Kind::LogManagement => "Log management",
            Kind::Alarm => "Alarm events",
            Kind::AlarmPeriod => "Alarm period report",
            Kind::ManualTest => "Manual static tests",
            Kind::AutoTest => "Automatic static tests",
            Kind::Csld => "CSLD",
            Kind::LineTest => "Line leak tests",
            Kind::LineDiag => "Line leak diagnostics",
            Kind::Compliance => "Compliance records",
            Kind::Delivery => "Deliveries",
            Kind::Inventory => "Inventory and shifts",
            Kind::Power => "Power off and on",
            Kind::AppLog => "Application log",
            Kind::AutoEvent => "Auto events",
            Kind::ProgramLog => "Program log files",
            Kind::Isd => "ISD records",
        }
    }

    /// The Source column's text (also in saved and copied text).
    pub fn tag(self) -> &'static str {
        match self {
            Kind::Audit => "AUDIT",
            Kind::LogManagement => "LOG MGMT",
            Kind::Alarm => "ALARM",
            Kind::AlarmPeriod => "ALARM RPT",
            Kind::ManualTest | Kind::AutoTest => "TANK TEST",
            Kind::Csld => "CSLD",
            Kind::LineTest => "LINE TEST",
            Kind::LineDiag => "LINE DIAG",
            Kind::Compliance => "COMPLIANCE",
            Kind::Delivery => "DELIVERY",
            Kind::Inventory => "INVENTORY",
            Kind::Power => "POWER",
            Kind::AppLog => "APP LOG",
            Kind::AutoEvent => "AUTO EVENT",
            Kind::ProgramLog => "PROGRAM",
            Kind::Isd => "ISD",
        }
    }

    /// Ticked when a snapshot opens and after Reset filters / Defaults: what a
    /// person did and what the console reported, without the bulk records.
    pub fn default_on(self) -> bool {
        matches!(
            self,
            Kind::Audit | Kind::Alarm | Kind::ManualTest | Kind::LineTest | Kind::Delivery | Kind::Power
        )
    }

    /// Hover text for the tick box: where the rows come from and what they hold.
    pub fn about(self) -> &'static str {
        match self {
            Kind::Audit => "Logins, logouts, setting changes and alarm acknowledgements, with the user for each.",
            Kind::LogManagement => {
                "The console's Log Management report. Most of its entries repeat the audit trail: \
                 while Audit trail is ticked, those are shown once (as audit rows)."
            }
            Kind::Alarm => "Each alarm posted, acknowledged and cleared, with the console's alarm name.",
            Kind::AlarmPeriod => "The alarm period report's records: each device's alarm status at the report time.",
            Kind::ManualTest => {
                "Static leak tests started at the console. One row when a test starts, one when it ends (with its result). The console does not record who starts them."
            }
            Kind::AutoTest => {
                "Static leak tests the console ran on its own. One row when a test starts, one when it ends (with its result)."
            }
            Kind::Csld => "Continuous statistical leak detection: each rate test, and each change of a tank's CSLD result.",
            Kind::LineTest => "Line leak (PLLD) test results as the console reports them: 3.0, 0.2 or 0.1 gph, passed or failed.",
            Kind::LineDiag => {
                "PLLD test details: pressures, ramp rates and fail ratios, port pressure checks, \
                 no-spike checks and auto-confirm periods. Many rows."
            }
            Kind::Compliance => "Monthly compliance records and each tank's and line's last passed test.",
            Kind::Delivery => {
                "Each delivery's start and end, with volumes, heights and temperatures. Also deliveries during a power outage."
            }
            Kind::Inventory => "Inventory readings, shift closes, power outage inventory and generator inventory.",
            Kind::Power => "Each time the console lost power and came back on.",
            Kind::AppLog => "The console's application log: system messages such as rejected CSLD rate tests.",
            Kind::AutoEvent => "Automatic reports and dial-outs the console sent, with the result of each.",
            Kind::ProgramLog => {
                "Lines from the console's program log files (opt/atg/logs, except status.log), with the file as the device. \
                 Times are as the console wrote them: lines written before its time zone was set may be in UTC."
            }
            Kind::Isd => "In-station diagnostics (ISD) history records.",
        }
    }
}

/// Which kinds are ticked, by `Kind::index`.
pub type KindSet = [bool; Kind::COUNT];

pub fn defaults() -> KindSet {
    Kind::ALL.map(Kind::default_on)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_is_in_one_group_in_order() {
        let listed: Vec<Kind> = GROUPS.iter().flat_map(|g| g.1.iter().copied()).collect();
        assert_eq!(listed, Kind::ALL.to_vec());
        for (i, k) in Kind::ALL.iter().enumerate() {
            assert_eq!(k.index(), i);
        }
    }

    #[test]
    fn tags_fit_the_source_column() {
        for k in Kind::ALL {
            assert!(k.tag().len() <= 10, "{}", k.tag());
        }
    }
}
