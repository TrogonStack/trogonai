use serde::{Deserialize, Serialize};

use crate::StrippedHeader;

/// Response headers removed before the sandbox sees a reply.
///
/// An empty list means the reply passes through untouched. It serializes as a JSON array of folded header names.
/// A name that repeats is kept once, in first-seen order, so construction and deserialization both drop repeats.
#[derive(Debug, Clone, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "Vec<StrippedHeader>", into = "Vec<StrippedHeader>")]
pub struct ResponseHeaderStrip(Vec<StrippedHeader>);

impl ResponseHeaderStrip {
    pub fn new(headers: impl IntoIterator<Item = StrippedHeader>) -> Self {
        let mut unique: Vec<StrippedHeader> = Vec::new();
        for header in headers {
            if !unique.contains(&header) {
                unique.push(header);
            }
        }
        Self(unique)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn headers(&self) -> &[StrippedHeader] {
        &self.0
    }
}

impl From<Vec<StrippedHeader>> for ResponseHeaderStrip {
    fn from(headers: Vec<StrippedHeader>) -> Self {
        Self::new(headers)
    }
}

impl From<ResponseHeaderStrip> for Vec<StrippedHeader> {
    fn from(strip: ResponseHeaderStrip) -> Self {
        strip.0
    }
}
