use super::{AgentCharter, AgentName, Annotations, Labels, ParentRef, Principal};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDefinition {
    name: AgentName,
    parent: ParentRef,
    owner: Principal,
    labels: Labels,
    annotations: Annotations,
    charter: AgentCharter,
}

impl AgentDefinition {
    pub fn new(
        name: AgentName,
        parent: ParentRef,
        owner: Principal,
        labels: Labels,
        annotations: Annotations,
        charter: AgentCharter,
    ) -> Self {
        Self {
            name,
            parent,
            owner,
            labels,
            annotations,
            charter,
        }
    }

    pub fn name(&self) -> &AgentName {
        &self.name
    }

    pub fn parent(&self) -> &ParentRef {
        &self.parent
    }

    pub fn owner(&self) -> &Principal {
        &self.owner
    }

    pub fn labels(&self) -> &Labels {
        &self.labels
    }

    pub fn annotations(&self) -> &Annotations {
        &self.annotations
    }

    pub fn charter(&self) -> &AgentCharter {
        &self.charter
    }
}

#[cfg(test)]
mod tests;
