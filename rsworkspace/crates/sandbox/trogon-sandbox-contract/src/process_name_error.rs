/// A process name does not satisfy the rules [`ProcessName`](crate::ProcessName) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum ProcessNameError {
    #[error("process name must be 1 to {max} characters")]
    Length { max: usize },
    #[error("process name may only contain ASCII letters, digits, '.', '_' and '-'")]
    Characters,
}
