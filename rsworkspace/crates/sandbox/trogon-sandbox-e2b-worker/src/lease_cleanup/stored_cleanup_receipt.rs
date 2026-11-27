use super::phase::Phase;
use super::receipt::Receipt;

/// A receipt as read, with the revision a compare-and-swap update must name.
pub(crate) struct StoredCleanupReceipt {
    pub(super) receipt: Receipt,
    pub(super) revision: u64,
}

impl StoredCleanupReceipt {
    /// Whether the sandbox is already confirmed gone.
    pub(crate) fn absent(&self) -> bool {
        self.receipt.phase == Phase::Absent
    }
}
