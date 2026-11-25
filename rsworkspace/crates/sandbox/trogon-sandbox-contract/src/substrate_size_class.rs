use serde::{Deserialize, Serialize};

use crate::SubstrateSizeClassError;
use crate::substrate_size_class_wire::SubstrateSizeClassWire;

/// One memory-tiered worker pool, named by the label value the substrate deployment stamps on its
/// pools.
///
/// Construction and deserialization reject an empty label and zero memory.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "SubstrateSizeClassWire")]
pub struct SubstrateSizeClass {
    label: String,
    memory_mib: u32,
}

impl SubstrateSizeClass {
    pub fn new(label: impl Into<String>, memory_mib: u32) -> Result<Self, SubstrateSizeClassError> {
        let label = label.into();
        if label.is_empty() {
            return Err(SubstrateSizeClassError::EmptyLabel);
        }
        if memory_mib == 0 {
            return Err(SubstrateSizeClassError::ZeroMemory { label });
        }
        Ok(Self { label, memory_mib })
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn memory_mib(&self) -> u32 {
        self.memory_mib
    }
}

impl TryFrom<SubstrateSizeClassWire> for SubstrateSizeClass {
    type Error = SubstrateSizeClassError;

    fn try_from(wire: SubstrateSizeClassWire) -> Result<Self, Self::Error> {
        Self::new(wire.label, wire.memory_mib)
    }
}
