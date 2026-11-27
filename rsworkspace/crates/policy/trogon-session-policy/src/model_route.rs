use crate::Destination;

/// The host's exact destination and model for one purpose.
#[derive(Debug, Clone)]
pub struct ModelRoute {
    pub(crate) destination: Destination,
    pub(crate) model: String,
}

impl ModelRoute {
    pub fn destination(&self) -> &Destination {
        &self.destination
    }
    pub fn model(&self) -> &str {
        &self.model
    }
}
