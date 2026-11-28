use std::fmt;

use trogon_peer::BatchId;

use crate::model_text::quoted;

/// Names the batch still waiting for acknowledgment, if this connector delivered one.
pub(crate) struct WaitingBatch<'a>(pub(crate) Option<&'a BatchId>);

impl fmt::Display for WaitingBatch<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(batch) => write!(
                formatter,
                "The batch waiting for acknowledgment is {}.",
                quoted(batch.as_str())
            ),
            None => write!(formatter, "No batch is waiting for acknowledgment."),
        }
    }
}
