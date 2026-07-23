use super::{DelegateSelectors, ModelId, ModelParameters, RuntimeId, ToolSelectors};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCharter {
    runtime: RuntimeId,
    model_id: ModelId,
    model_params: ModelParameters,
    tools: ToolSelectors,
    delegates: DelegateSelectors,
}

impl AgentCharter {
    pub const fn new(
        runtime: RuntimeId,
        model_id: ModelId,
        model_params: ModelParameters,
        tools: ToolSelectors,
        delegates: DelegateSelectors,
    ) -> Self {
        Self {
            runtime,
            model_id,
            model_params,
            tools,
            delegates,
        }
    }

    pub const fn runtime(&self) -> &RuntimeId {
        &self.runtime
    }

    pub const fn model_id(&self) -> &ModelId {
        &self.model_id
    }

    pub const fn model_params(&self) -> &ModelParameters {
        &self.model_params
    }

    pub const fn tools(&self) -> &ToolSelectors {
        &self.tools
    }

    pub const fn delegates(&self) -> &DelegateSelectors {
        &self.delegates
    }
}

#[cfg(test)]
mod tests;
