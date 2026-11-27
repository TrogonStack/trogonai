use trogon_sandbox_contract::SnapshotAccountingRefusal;

/// Lets tests propagate a snapshot accounting refusal with `?`.
pub(crate) trait Accepted<T> {
    fn accepted(self) -> Result<T, Box<dyn std::error::Error>>;
}

impl<T> Accepted<T> for Result<T, SnapshotAccountingRefusal> {
    fn accepted(self) -> Result<T, Box<dyn std::error::Error>> {
        self.map_err(|refusal| format!("snapshot accounting refused: {refusal:?}").into())
    }
}
