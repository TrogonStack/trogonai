use std::fmt;

use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use serde_json::Value;
use tokio::sync::Mutex;
use trogon_peer_context::{PeerContext, PeerContextError};
use trogonai_proto::peers::v1alpha1 as v1;

use crate::connector_config::ConnectorConfig;
use crate::native_binding::NativeBinding;
use crate::prompt_hook_context::PromptHookContext;
use crate::prompt_hook_event::PromptHookEvent;
use crate::prompt_hook_input::PromptHookInput;
use crate::prompt_hook_output::PromptHookOutput;
use crate::prompt_tool::prompt_tool;
use crate::{ConnectorConfigInput, EnrollPeer, PeerMcpError, PreparePeerBatch};

pub struct PeerMcp<B> {
    backend: B,
    config: ConnectorConfig,
    binding: Mutex<Option<NativeBinding>>,
}

impl<B> fmt::Debug for PeerMcp<B> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("PeerMcp").finish_non_exhaustive()
    }
}

impl<B: EnrollPeer + PreparePeerBatch> PeerMcp<B> {
    pub fn new(backend: B, config: ConnectorConfigInput) -> Result<Self, PeerMcpError> {
        let config = ConnectorConfig::try_from(config).map_err(PeerMcpError::InvalidConfig)?;
        Ok(Self {
            backend,
            config,
            binding: Mutex::new(None),
        })
    }

    pub async fn invoke(&self, request: CallToolRequestParams) -> Result<CallToolResult, PeerMcpError> {
        match request.name.as_ref() {
            "peer_prompt_hook" => self.prompt_hook(request).await,
            "peer_roster" | "peer_send" | "peer_status" | "peer_acknowledge_batch" => {
                let binding = self.binding.lock().await;
                if binding.as_ref().and_then(NativeBinding::attachment).is_none() {
                    Err(PeerMcpError::Unbound)
                } else {
                    Err(PeerMcpError::UnsupportedTool)
                }
            }
            _ => Err(PeerMcpError::UnsupportedTool),
        }
    }

    async fn prompt_hook(&self, request: CallToolRequestParams) -> Result<CallToolResult, PeerMcpError> {
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        let input: PromptHookInput = serde_json::from_value(arguments).map_err(PeerMcpError::InvalidArguments)?;
        let (session_id, prompt_id) = input.into_identity()?;
        let mut binding = self.binding.lock().await;
        if binding
            .as_ref()
            .is_some_and(|current| current.native_session_id() != &session_id)
        {
            return Err(PeerMcpError::NativeSessionChanged);
        }
        let current = binding.get_or_insert_with(|| NativeBinding::new(session_id));
        let enrollment = self
            .backend
            .enroll(v1::EnrollRequest {
                installation_id: Some(self.config.installation_id().as_str().to_owned()),
                native_session_id: Some(current.native_session_id().as_str().to_owned()),
                connector_id: Some(self.config.connector_id().as_str().to_owned()),
                collaboration_ref: Some(self.config.collaboration_ref().as_str().to_owned()),
                display_name: Some(self.config.display_name().as_str().to_owned()),
            })
            .await
            .map_err(|error| PeerMcpError::Enroll(Box::new(error)))?;
        let attachment = current.revalidate(&enrollment, &self.config)?;
        let response = self
            .backend
            .prepare_batch(v1::PrepareBatchRequest {
                attachment_ref: v1::AttachmentRef {
                    peer_id: Some(attachment.peer_id().as_str().to_owned()),
                    connector_id: Some(attachment.connector_id().as_str().to_owned()),
                    generation: Some(attachment.generation()),
                }
                .into(),
                prompt_id: prompt_id.map(|id| id.as_str().to_owned()),
            })
            .await
            .map_err(|error| PeerMcpError::Prepare(Box::new(error)))?;
        let context = match response.batch.as_option() {
            None if response.messages.is_empty() => String::new(),
            None => return Err(contract_error()),
            Some(batch) => {
                match response
                    .content_availability
                    .as_ref()
                    .and_then(|availability| availability.as_known())
                {
                    Some(v1::ContentAvailability::CONTENT_AVAILABILITY_AVAILABLE) => {}
                    Some(v1::ContentAvailability::CONTENT_AVAILABILITY_RETRY_UNAVAILABLE) => {
                        return Err(PeerMcpError::RetryContentUnavailable);
                    }
                    _ => return Err(contract_error()),
                }
                for message in &response.messages {
                    let metadata = message.metadata.as_option().ok_or_else(contract_error)?;
                    if metadata.recipient_peer_id.as_deref() != Some(attachment.peer_id().as_str())
                        || metadata.collaboration_ref.as_deref() != Some(self.config.collaboration_ref().as_str())
                    {
                        return Err(contract_error());
                    }
                }
                PeerContext::render(batch, &response.messages)
                    .map_err(PeerMcpError::InvalidBatch)?
                    .as_str()
                    .to_owned()
            }
        };
        let output = PromptHookOutput {
            hook_specific_output: PromptHookContext {
                hook_event_name: PromptHookEvent::UserPromptSubmit,
                additional_context: &context,
            },
        };
        let value = serde_json::to_value(output).map_err(PeerMcpError::Encode)?;
        let text = serde_json::to_string(&value).map_err(PeerMcpError::Encode)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }
}

fn contract_error() -> PeerMcpError {
    PeerMcpError::InvalidBatch(PeerContextError::ContractViolation)
}

impl<B: EnrollPeer + PreparePeerBatch> ServerHandler for PeerMcp<B> {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        (name == "peer_prompt_hook").then(prompt_tool)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(vec![prompt_tool()]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let result = match self.invoke(request).await {
            Ok(result) => result,
            Err(error) => CallToolResult::error(vec![ContentBlock::text(error.to_string())]),
        };
        Ok(CallToolResponse::Complete(result))
    }
}
