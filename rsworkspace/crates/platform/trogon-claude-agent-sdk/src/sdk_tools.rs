use std::{
    collections::{BTreeMap, btree_map::Entry},
    sync::Arc,
};

use crate::{McpToolName, SdkTool};

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SdkToolsError {
    #[error("duplicate MCP tool name: {name}")]
    DuplicateTool { name: McpToolName },
}

#[derive(Clone, Debug, Default)]
pub struct SdkTools {
    tools: Arc<BTreeMap<String, SdkTool>>,
}

impl SdkTools {
    pub fn new(tools: impl IntoIterator<Item = SdkTool>) -> Result<Self, SdkToolsError> {
        let mut mapped = BTreeMap::new();
        for tool in tools {
            match mapped.entry(tool.name().as_str().to_owned()) {
                Entry::Occupied(_) => {
                    return Err(SdkToolsError::DuplicateTool {
                        name: tool.name().clone(),
                    });
                }
                Entry::Vacant(entry) => {
                    entry.insert(tool);
                }
            }
        }
        Ok(Self {
            tools: Arc::new(mapped),
        })
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub(crate) fn get(&self, name: &str) -> Option<&SdkTool> {
        self.tools.get(name)
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &SdkTool> {
        self.tools.values()
    }
}

impl From<SdkTool> for SdkTools {
    fn from(tool: SdkTool) -> Self {
        Self {
            tools: Arc::new(BTreeMap::from([(tool.name().as_str().to_owned(), tool)])),
        }
    }
}
