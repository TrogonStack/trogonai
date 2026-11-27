use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::SnapshotPublished;

use super::capture_binding::CaptureBinding;
use super::capture_reservation::CaptureReservation;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct CaptureRecord {
    pub(super) reservation: CaptureReservation,
    pub(super) binding: Option<CaptureBinding>,
    pub(super) abandoned: bool,
    #[serde(default)]
    pub(super) sealed: bool,
    #[serde(default)]
    pub(super) intent: Option<SnapshotPublished>,
    #[serde(default)]
    pub(super) enqueued: bool,
}
