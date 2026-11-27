use trogon_sandbox_contract::OperationRecord;

/// The outcome of a write that needed to still hold its claim.
#[derive(Debug)]
pub enum Settled {
    Settled(Box<OperationRecord>),
    /// The claim moved to another owner or generation; nothing was written.
    Fenced,
}
