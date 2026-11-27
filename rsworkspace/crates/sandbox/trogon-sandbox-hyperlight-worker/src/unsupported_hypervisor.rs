use std::path::Path;

use crate::{Hypervisor, HypervisorError};

/// The backend of a host that has no hypervisor: it is never available and every operation
/// reports [`HypervisorError::Unavailable`].
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedHypervisor;

impl Hypervisor for UnsupportedHypervisor {
    type Warm = ();
    type Guest = ();

    fn is_available(&self) -> bool {
        false
    }

    fn build_warm(&self) -> Result<Self::Warm, HypervisorError> {
        Err(HypervisorError::Unavailable)
    }

    fn load_module(&self, _warm: Self::Warm, _component: &Path) -> Result<Self::Guest, HypervisorError> {
        Err(HypervisorError::Unavailable)
    }

    fn load_layout(&self, _warm: Self::Warm, _layout: &Path) -> Result<Self::Guest, HypervisorError> {
        Err(HypervisorError::Unavailable)
    }

    fn save_layout(&self, _guest: &mut Self::Guest, _layout: &Path) -> Result<(), HypervisorError> {
        Err(HypervisorError::Unavailable)
    }

    fn unload(&self, _guest: Self::Guest) -> Result<Self::Warm, HypervisorError> {
        Err(HypervisorError::Unavailable)
    }
}
