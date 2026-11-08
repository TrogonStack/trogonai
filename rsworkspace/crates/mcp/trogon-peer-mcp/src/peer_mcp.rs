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
use crate::model_tool_definitions::model_tool;
use crate::model_tools;
use crate::native_binding::NativeBinding;
use crate::prompt_hook_context::PromptHookContext;
use crate::prompt_hook_event::PromptHookEvent;
use crate::prompt_hook_input::PromptHookInput;
use crate::prompt_hook_output::PromptHookOutput;
use crate::prompt_tool::prompt_tool;
use crate::{
    AcknowledgePeerBatch, ConnectorConfigInput, EnrollPeer, PeerMcpError, PreparePeerBatch, ReadPeerMessageStatus,
    ReadPeerRoster, SendPeerMessage,
};

pub struct PeerMcp<B> {
    backend: B,
    config: ConnectorConfig,
    binding: Mutex<Option<NativeBinding>>,
}

enum ModelOperation {
    Roster,
    Send,
    Status,
    Acknowledge,
}

impl<B> fmt::Debug for PeerMcp<B> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("PeerMcp").finish_non_exhaustive()
    }
}

impl<B> PeerMcp<B>
where
    B: EnrollPeer + PreparePeerBatch + ReadPeerRoster + SendPeerMessage + ReadPeerMessageStatus + AcknowledgePeerBatch,
{
    pub fn new(backend: B, config: ConnectorConfigInput) -> Result<Self, PeerMcpError> {
        let config = ConnectorConfig::try_from(config).map_err(PeerMcpError::InvalidConfig)?;
        Ok(Self {
            backend,
            config,
            binding: Mutex::new(None),
        })
    }

    pub async fn invoke(&self, request: CallToolRequestParams) -> Result<CallToolResult, PeerMcpError> {
        let operation = match request.name.as_ref() {
            "peer_prompt_hook" => return self.prompt_hook(request).await,
            "peer_roster" => ModelOperation::Roster,
            "peer_send" => ModelOperation::Send,
            "peer_status" => ModelOperation::Status,
            "peer_acknowledge_batch" => ModelOperation::Acknowledge,
            _ => return Err(PeerMcpError::UnsupportedTool),
        };
        let attachment = self
            .binding
            .lock()
            .await
            .as_ref()
            .and_then(NativeBinding::attachment)
            .cloned()
            .ok_or(PeerMcpError::Unbound)?;
        match operation {
            ModelOperation::Roster => model_tools::roster(&self.backend, request, &attachment).await,
            ModelOperation::Send => model_tools::send(&self.backend, request, &attachment, &self.config).await,
            ModelOperation::Status => model_tools::status(&self.backend, request, &attachment, &self.config).await,
            ModelOperation::Acknowledge => model_tools::acknowledge(&self.backend, request, &attachment).await,
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

impl<B> ServerHandler for PeerMcp<B>
where
    B: EnrollPeer + PreparePeerBatch + ReadPeerRoster + SendPeerMessage + ReadPeerMessageStatus + AcknowledgePeerBatch,
{
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        if name == "peer_prompt_hook" {
            Some(prompt_tool())
        } else {
            model_tool(name)
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut tools = vec![prompt_tool()];
        tools.extend(
            ["peer_roster", "peer_send", "peer_status", "peer_acknowledge_batch"]
                .into_iter()
                .filter_map(model_tool),
        );
        Ok(ListToolsResult::with_all_items(tools))
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
