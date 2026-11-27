use serde::{Deserialize, Serialize};

/// Whether the platform keeps a lease's command output history after the lease ends.
///
/// With `Discarded`, output still streams live and logs stay readable while the lease runs, but the stored history
/// is purged when the lease ends. It serializes as a JSON boolean, `true` for `Retained`. An omitted value reads as
/// `Retained`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum MonitoringRetention {
    #[default]
    Retained,
    Discarded,
}

impl MonitoringRetention {
    pub const fn is_retained(self) -> bool {
        matches!(self, Self::Retained)
    }
}

impl From<bool> for MonitoringRetention {
    fn from(retained: bool) -> Self {
        if retained { Self::Retained } else { Self::Discarded }
    }
}

impl From<MonitoringRetention> for bool {
    fn from(retention: MonitoringRetention) -> Self {
        retention.is_retained()
    }
}
