use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::AttemptQuotaReservation;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum AttemptQuotaPhase {
    Held,
    Released,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct AttemptQuotaReceipt {
    pub(crate) reservation: AttemptQuotaReservation,
    pub(crate) phase: AttemptQuotaPhase,
}
