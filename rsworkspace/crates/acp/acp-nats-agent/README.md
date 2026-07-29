# acp-nats-agent

Server-side framework for building [ACP](https://agentclientprotocol.com/) agents over NATS.

## Architecture

```mermaid
graph LR
    IDE <--> Bridge["Bridge (acp-nats-stdio)"] <--> NATS <--> Agent["Agent (acp-nats-agent)"]
```

## Usage

Runners implement `acp_nats::AgentHandler`. The SDK's own `Agent` trait was removed in
`agent-client-protocol` 2.0.0, so the bridge owns this trait surface
(see [ADR#0020](../../../../docs/adr/0020-acp-sdk-1x-boundary-and-bridge-traits.md)).
Only `initialize`, `authenticate`, `new_session`, `prompt`, and `cancel` are required; every
other method defaults to `method_not_found`.

```rust,no_run
use acp_nats::{AcpPrefix, AgentHandler};
use acp_nats_agent::AgentSideNatsConnection;
use agent_client_protocol::Result;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, StopReason,
};

struct MyAgent;

#[async_trait::async_trait]
impl AgentHandler for MyAgent {
    async fn initialize(&self, _args: InitializeRequest) -> Result<InitializeResponse> {
        Ok(InitializeResponse::new(ProtocolVersion::V0))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, _args: NewSessionRequest) -> Result<NewSessionResponse> {
        Ok(NewSessionResponse::new("session-123"))
    }

    async fn prompt(&self, _args: PromptRequest) -> Result<PromptResponse> {
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(&self, _args: CancelNotification) -> Result<()> {
        Ok(())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let nats = async_nats::connect("localhost:4222").await.unwrap();

    let (_connection, io_task) = AgentSideNatsConnection::new(
        MyAgent,
        nats,
        AcpPrefix::new("acp").unwrap(),
        |fut| {
            tokio::task::spawn_local(fut);
        },
    );

    let local = tokio::task::LocalSet::new();
    local.run_until(io_task).await.unwrap();
}
```
