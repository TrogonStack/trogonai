//! Image bytes returned directly by a tool.

use serde::{Deserialize, Serialize};

/// An image returned by a tool, separate from user file attachments.
///
/// The data remains base64 encoded as it arrived over the tool protocol. This
/// is content to deliver to the model, never authority to read a local path.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolImage {
    /// Base64-encoded image bytes.
    pub data: String,
    /// The image media type.
    pub mime_type: String,
}
