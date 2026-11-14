use crate::{ConfigurationRevision, EndpointRegistration};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredEndpoint {
    registration: EndpointRegistration,
    configuration_revision: ConfigurationRevision,
}

impl RegisteredEndpoint {
    pub fn new(registration: EndpointRegistration, configuration_revision: ConfigurationRevision) -> Self {
        Self {
            registration,
            configuration_revision,
        }
    }

    pub fn registration(&self) -> &EndpointRegistration {
        &self.registration
    }
    pub fn configuration_revision(&self) -> ConfigurationRevision {
        self.configuration_revision
    }
}
