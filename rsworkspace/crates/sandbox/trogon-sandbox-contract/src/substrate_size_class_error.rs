/// A substrate size class or size class ladder that cannot be built.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum SubstrateSizeClassError {
    #[error("at least one size class is required")]
    Empty,
    #[error("size class label must not be empty")]
    EmptyLabel,
    #[error("size class `{label}` must declare non-zero memory")]
    ZeroMemory { label: String },
    #[error("two size classes declare {memory_mib} MiB")]
    DuplicateMemory { memory_mib: u32 },
    #[error("size class `{entry}` is not in `label=memory_mib` form")]
    MalformedEntry { entry: String },
    #[error("size class `{entry}` has a non-numeric memory value")]
    MalformedMemory {
        entry: String,
        #[source]
        source: std::num::ParseIntError,
    },
}
