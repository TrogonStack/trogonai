use serde::Deserialize;

/// Unchecked resource bounds as they arrive on the wire.
#[derive(Deserialize)]
pub(crate) struct ResourceRangeWire {
    pub(crate) min: u32,
    pub(crate) max: u32,
}
