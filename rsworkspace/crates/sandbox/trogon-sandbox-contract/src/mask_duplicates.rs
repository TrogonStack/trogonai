use serde::{Deserialize, Serialize};

/// Whether a masked file gives each distinct extracted value its own stand-in.
///
/// With an extract rule on a masked file, `Enabled` gives each distinct extracted value its own stand-in and also
/// replaces every verbatim copy of an extracted value elsewhere in the file. A value that differs from the one
/// fetched for the binding is itself treated as a credential to swap back on the inject hosts. `Disabled` uses one
/// stand-in for every extracted span and leaves copies outside the spans as they were.
///
/// It serializes as a JSON boolean, `true` for `Enabled`, so existing peers keep parsing it.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum MaskDuplicates {
    Enabled,
    #[default]
    Disabled,
}

impl MaskDuplicates {
    pub const fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }

    /// Reports whether this is the default, so a binding can omit it from the wire.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl From<bool> for MaskDuplicates {
    fn from(enabled: bool) -> Self {
        if enabled { Self::Enabled } else { Self::Disabled }
    }
}

impl From<MaskDuplicates> for bool {
    fn from(mode: MaskDuplicates) -> Self {
        mode.is_enabled()
    }
}
