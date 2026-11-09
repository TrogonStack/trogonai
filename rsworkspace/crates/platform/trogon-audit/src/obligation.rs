/// What a writer owes a record before and after the act it describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Obligation {
    /// A must-audit record of an act that has not happened yet: acknowledged
    /// before the effect, or the act is refused as an operational fault.
    BeforeEffect,
    /// A must-audit record of an act that was refused. The refusal stands whether
    /// or not the record is written, so a failed write is buffered and retried.
    Stands,
    /// Queued and published asynchronously; dropping it never refuses anything.
    BestEffort,
}
