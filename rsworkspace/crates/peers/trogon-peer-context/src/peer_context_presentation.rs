use serde::Serialize;

use crate::context_message_presentation::ContextMessagePresentation;

#[derive(Serialize)]
pub(crate) struct PeerContextPresentation<'a> {
    pub(crate) batch_id: &'a str,
    pub(crate) messages: Vec<ContextMessagePresentation<'a>>,
}
