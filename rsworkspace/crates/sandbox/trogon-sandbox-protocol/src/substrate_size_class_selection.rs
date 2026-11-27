pub use crate::constants::DEFAULT_SUBSTRATE_SIZE_CLASSES;
use trogon_sandbox_contract::{SubstrateSizeClass, SubstrateSizeClassError, SubstrateSizeClasses};

/// The default ladder of Substrate pools.
pub fn default_substrate_size_classes() -> Result<SubstrateSizeClasses, SubstrateSizeClassError> {
    DEFAULT_SUBSTRATE_SIZE_CLASSES.parse()
}

/// The smallest class that holds `memory_mib`, or `None` when the request exceeds every pool.
pub fn select_substrate_size_class(classes: &SubstrateSizeClasses, memory_mib: u32) -> Option<&SubstrateSizeClass> {
    classes.as_slice().iter().find(|class| class.memory_mib() >= memory_mib)
}

/// The memory of the largest class, which is the ceiling a lease can request.
pub fn max_substrate_memory_mib(classes: &SubstrateSizeClasses) -> u32 {
    classes
        .as_slice()
        .last()
        .map(SubstrateSizeClass::memory_mib)
        .unwrap_or_default()
}
