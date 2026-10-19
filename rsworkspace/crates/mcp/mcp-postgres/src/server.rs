use crate::QueryService;
use rmcp::schemars;
use rmcp::{
    ErrorData, ServerHandler,
    model::*,
    service::{RequestContext, RoleServer},
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Clone)]
pub struct PostgresMcp {
    service: Arc<QueryService>,
}
impl PostgresMcp {
    pub fn new(service: QueryService) -> Self {
        Self {
            service: Arc::new(service),
        }
    }
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct QueryRequest {
    connection: String,
    #[schemars(length(max = 16384))]
    sql: String,
    #[serde(default)]
    #[schemars(length(max = 32))]
    parameters: Vec<crate::ParameterInput>,
}
impl ServerHandler for PostgresMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions("Queries use registered connections and the launcher's trusted identity. Only the documented bounded SELECT subset is supported.")
    }
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let input_schema = rmcp::handler::server::common::schema_for_type::<QueryRequest>();
        Ok(ListToolsResult {
            tools: vec![Tool::new(
                "query",
                "Query a registered PostgreSQL connection with column authorization",
                input_schema.as_ref().clone(),
            )],
            ..Default::default()
        })
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name != "query" {
            return Err(ErrorData::invalid_params("unknown tool", None));
        }
        let args: QueryRequest =
            serde_json::from_value(serde_json::Value::Object(request.arguments.unwrap_or_default()))
                .map_err(|_| ErrorData::invalid_params("invalid query arguments", None))?;
        let parameters = args
            .parameters
            .into_iter()
            .map(crate::Parameter::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ErrorData::invalid_params("invalid parameters", None))?;
        let outcome = tokio::select! {
            biased;
            _ = context.ct.cancelled() => return Err(ErrorData::internal_error("request cancelled",None)),
            outcome = self.service.query_with_parameters(&args.connection,&args.sql,&parameters) => outcome,
        };
        let result = match outcome {
            Ok(result) => CallToolResult::success(vec![ContentBlock::text(
                serde_json::to_string(&result)
                    .map_err(|_| ErrorData::internal_error("result encoding failed", None))?,
            )]),
            Err(error) => {
                let message = match error {
                    crate::Error::Denied => "access denied",
                    crate::Error::Limit => "resource limit exceeded",
                    crate::Error::Timeout => "request timed out",
                    crate::Error::UnsupportedSql | crate::Error::Parse(_) | crate::Error::Identifier => {
                        "unsupported or invalid SQL"
                    }
                    crate::Error::UnknownConnection => "unknown connection",
                    _ => "query unavailable",
                };
                CallToolResult::error(vec![ContentBlock::text(message)])
            }
        };
        Ok(CallToolResponse::Complete(result))
    }
}
