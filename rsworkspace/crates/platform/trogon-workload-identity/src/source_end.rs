use crate::source_fault::SourceFault;

/// How one Workload API connection ended. `observed` is true when it supplied
/// at least one acceptable response before it failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourceEnd {
    pub fault: SourceFault,
    pub observed: bool,
}

impl SourceEnd {
    pub(crate) const fn new(fault: SourceFault, observed: bool) -> Self {
        Self { fault, observed }
    }
}
