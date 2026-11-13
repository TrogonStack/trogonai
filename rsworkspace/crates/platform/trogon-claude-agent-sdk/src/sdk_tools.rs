use std::{collections::HashSet, sync::Arc};

use crate::{McpToolName, SdkTool};

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SdkToolsError {
    #[error("Tool {name} is already registered")]
    DuplicateTool { name: McpToolName },
}

/// The tools of one in-process MCP server, in registration order.
#[derive(Clone, Debug, Default)]
pub struct SdkTools {
    tools: Arc<Vec<SdkTool>>,
}

impl SdkTools {
    /// Rejects a repeated name like the TypeScript `McpServer.registerTool`.
    pub fn new(tools: impl IntoIterator<Item = SdkTool>) -> Result<Self, SdkToolsError> {
        let mut registered: Vec<SdkTool> = Vec::new();
        for tool in tools {
            if registered.iter().any(|existing| existing.name() == tool.name()) {
                return Err(SdkToolsError::DuplicateTool {
                    name: tool.name().clone(),
                });
            }
            registered.push(tool);
        }
        Ok(Self {
            tools: Arc::new(registered),
        })
    }

    /// Keeps repeated names like the Python `create_sdk_mcp_server`: every definition is listed, and a call
    /// runs the last definition with that name.
    pub fn with_duplicates(tools: impl IntoIterator<Item = SdkTool>) -> Self {
        Self {
            tools: Arc::new(tools.into_iter().collect()),
        }
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// The last tool registered under `name`.
    pub(crate) fn get(&self, name: &str) -> Option<&SdkTool> {
        self.tools.iter().rev().find(|tool| tool.name().as_str() == name)
    }

    /// Every registered definition, in registration order.
    pub(crate) fn values(&self) -> impl Iterator<Item = &SdkTool> {
        self.tools.iter()
    }

    /// The order a JavaScript object keyed by tool name enumerates: canonical array-index names ascending,
    /// then the other names in registration order, each name once with its last definition.
    pub(crate) fn typescript_order(&self) -> Vec<&SdkTool> {
        let mut seen = HashSet::new();
        let mut listed: Vec<&SdkTool> = self
            .tools
            .iter()
            .filter(|tool| seen.insert(tool.name().as_str()))
            .filter_map(|tool| self.get(tool.name().as_str()))
            .collect();
        listed.sort_by_key(|tool| array_index(tool.name().as_str()).map_or((1, 0), |index| (0, index)));
        listed
    }
}

/// A canonical JavaScript array index: digits without a leading zero, below 2^32 - 1.
fn array_index(name: &str) -> Option<u32> {
    if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_digit()) || (name.len() > 1 && name.starts_with('0')) {
        return None;
    }
    name.parse::<u32>().ok().filter(|index| *index < u32::MAX)
}

impl From<SdkTool> for SdkTools {
    fn from(tool: SdkTool) -> Self {
        Self {
            tools: Arc::new(vec![tool]),
        }
    }
}
