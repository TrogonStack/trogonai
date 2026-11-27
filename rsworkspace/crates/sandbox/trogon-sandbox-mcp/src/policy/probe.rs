use trogon_sandbox_contract::FileKind;

/// What a metadata probe of one path found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    Kind(FileKind),
    Missing,
}
