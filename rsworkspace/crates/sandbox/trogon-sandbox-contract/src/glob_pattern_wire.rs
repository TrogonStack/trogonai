use serde::{Deserialize, Serialize};

use crate::LiteralPath;

/// The JSON shape of a [`GlobPattern`](crate::GlobPattern) before its grammar is checked.
///
/// A plain pattern is a JSON string. An anchored one is an object with an `anchor` and a `tail`.
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum GlobPatternWire {
    Plain(String),
    Anchored { anchor: LiteralPath, tail: String },
}
