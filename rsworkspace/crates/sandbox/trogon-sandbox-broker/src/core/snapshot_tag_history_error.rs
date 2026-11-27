/// Why a [`SnapshotTagHistoryReader`](super::SnapshotTagHistoryReader) call failed.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotTagHistoryError {
    #[error("snapshot tag history read failed: {message}")]
    Read { message: String },
}
