/// Whether the caller can open an unverifiable object for reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerReads {
    Yes,
    No,
    Unknown,
}
