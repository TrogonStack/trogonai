use serde::{Deserialize, Serialize};

use super::attempt_quota_receipt::AttemptQuotaReceipt;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct PendingAttemptQuota {
    pub(super) receipt: AttemptQuotaReceipt,
}
