use super::{AgentCharter, AgentName, ParentRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDefinition {
    name: AgentName,
    parent: ParentRef,
    charter: AgentCharter,
}

impl AgentDefinition {
    pub fn new(name: AgentName, parent: ParentRef, charter: AgentCharter) -> Self {
        Self { name, parent, charter }
    }

    pub fn name(&self) -> &AgentName {
        &self.name
    }

    pub fn parent(&self) -> &ParentRef {
        &self.parent
    }

    pub fn charter(&self) -> &AgentCharter {
        &self.charter
    }
}

#[cfg(test)]
mod tests;
