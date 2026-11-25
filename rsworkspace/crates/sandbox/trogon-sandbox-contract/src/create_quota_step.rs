use serde::{Deserialize, Serialize};

use crate::{AttemptQuotaReservation, LeaseError};

/// The quota intent a create retains until its effects are settled, tagged by `step`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "step")]
pub enum CreateQuotaStep {
    /// A reservation intent stored before admission. A missing receipt is legacy accounting that needs
    /// reconciliation.
    Reserved {
        cpu_seconds: u32,
        wall_seconds: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reservation: Option<AttemptQuotaReservation>,
    },
    Releasing {
        reservation: AttemptQuotaReservation,
        error: LeaseError,
    },
    /// The lease is recorded and counted, so nothing is left to reconcile.
    Counted,
}
