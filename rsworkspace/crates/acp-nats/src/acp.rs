//! ACP protocol surface used by `acp-nats`.
//!
//! `agent-client-protocol` 0.11+ removed the `Agent` / `Client` async traits in
//! favour of builder-based connections. The NATS bridge still needs a
//! method-per-RPC abstraction (the NATS side does its own serde and routing),
//! so this module re-exports the stable ACP v1 schema and defines the traits
//! locally, with the same shape as the 0.10 SDK traits.

use std::rc::Rc;
use std::sync::Arc;

pub use agent_client_protocol::schema::ProtocolVersion;
pub use agent_client_protocol::v1::schema::*;

/// The agent side of ACP: methods a client can call on an agent.
#[async_trait::async_trait(?Send)]
pub trait Agent {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse>;

    async fn authenticate(&self, args: AuthenticateRequest) -> Result<AuthenticateResponse>;

    async fn logout(&self, _args: LogoutRequest) -> Result<LogoutResponse> {
        Err(Error::method_not_found())
    }

    async fn new_session(&self, args: NewSessionRequest) -> Result<NewSessionResponse>;

    async fn prompt(&self, args: PromptRequest) -> Result<PromptResponse>;

    async fn cancel(&self, args: CancelNotification) -> Result<()>;

    async fn load_session(&self, _args: LoadSessionRequest) -> Result<LoadSessionResponse> {
        Err(Error::method_not_found())
    }

    async fn set_session_mode(&self, _args: SetSessionModeRequest) -> Result<SetSessionModeResponse> {
        Err(Error::method_not_found())
    }

    async fn set_session_config_option(
        &self,
        _args: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse> {
        Err(Error::method_not_found())
    }

    async fn list_sessions(&self, _args: ListSessionsRequest) -> Result<ListSessionsResponse> {
        Err(Error::method_not_found())
    }

    async fn fork_session(&self, _args: ForkSessionRequest) -> Result<ForkSessionResponse> {
        Err(Error::method_not_found())
    }

    async fn resume_session(&self, _args: ResumeSessionRequest) -> Result<ResumeSessionResponse> {
        Err(Error::method_not_found())
    }

    async fn close_session(&self, _args: CloseSessionRequest) -> Result<CloseSessionResponse> {
        Err(Error::method_not_found())
    }

    async fn ext_method(&self, _args: ExtRequest) -> Result<ExtResponse> {
        Ok(ExtResponse::new(RawValue::NULL.to_owned().into()))
    }

    async fn ext_notification(&self, _args: ExtNotification) -> Result<()> {
        Ok(())
    }
}

/// The client side of ACP: methods an agent can call on a client.
#[async_trait::async_trait(?Send)]
pub trait Client {
    async fn request_permission(&self, args: RequestPermissionRequest) -> Result<RequestPermissionResponse>;

    async fn session_notification(&self, args: SessionNotification) -> Result<()>;

    async fn write_text_file(&self, _args: WriteTextFileRequest) -> Result<WriteTextFileResponse> {
        Err(Error::method_not_found())
    }

    async fn read_text_file(&self, _args: ReadTextFileRequest) -> Result<ReadTextFileResponse> {
        Err(Error::method_not_found())
    }

    async fn create_terminal(&self, _args: CreateTerminalRequest) -> Result<CreateTerminalResponse> {
        Err(Error::method_not_found())
    }

    async fn terminal_output(&self, _args: TerminalOutputRequest) -> Result<TerminalOutputResponse> {
        Err(Error::method_not_found())
    }

    async fn release_terminal(&self, _args: ReleaseTerminalRequest) -> Result<ReleaseTerminalResponse> {
        Err(Error::method_not_found())
    }

    async fn wait_for_terminal_exit(&self, _args: WaitForTerminalExitRequest) -> Result<WaitForTerminalExitResponse> {
        Err(Error::method_not_found())
    }

    async fn kill_terminal(&self, _args: KillTerminalRequest) -> Result<KillTerminalResponse> {
        Err(Error::method_not_found())
    }

    async fn ext_method(&self, _args: ExtRequest) -> Result<ExtResponse> {
        Ok(ExtResponse::new(RawValue::NULL.to_owned().into()))
    }

    async fn ext_notification(&self, _args: ExtNotification) -> Result<()> {
        Ok(())
    }
}

macro_rules! forward_agent {
    ($ptr:ident) => {
        #[async_trait::async_trait(?Send)]
        impl<T: Agent + ?Sized> Agent for $ptr<T> {
            async fn initialize(&self, a: InitializeRequest) -> Result<InitializeResponse> {
                (**self).initialize(a).await
            }
            async fn authenticate(&self, a: AuthenticateRequest) -> Result<AuthenticateResponse> {
                (**self).authenticate(a).await
            }
            async fn logout(&self, a: LogoutRequest) -> Result<LogoutResponse> {
                (**self).logout(a).await
            }
            async fn new_session(&self, a: NewSessionRequest) -> Result<NewSessionResponse> {
                (**self).new_session(a).await
            }
            async fn prompt(&self, a: PromptRequest) -> Result<PromptResponse> {
                (**self).prompt(a).await
            }
            async fn cancel(&self, a: CancelNotification) -> Result<()> {
                (**self).cancel(a).await
            }
            async fn load_session(&self, a: LoadSessionRequest) -> Result<LoadSessionResponse> {
                (**self).load_session(a).await
            }
            async fn set_session_mode(&self, a: SetSessionModeRequest) -> Result<SetSessionModeResponse> {
                (**self).set_session_mode(a).await
            }
            async fn set_session_config_option(
                &self,
                a: SetSessionConfigOptionRequest,
            ) -> Result<SetSessionConfigOptionResponse> {
                (**self).set_session_config_option(a).await
            }
            async fn list_sessions(&self, a: ListSessionsRequest) -> Result<ListSessionsResponse> {
                (**self).list_sessions(a).await
            }
            async fn fork_session(&self, a: ForkSessionRequest) -> Result<ForkSessionResponse> {
                (**self).fork_session(a).await
            }
            async fn resume_session(&self, a: ResumeSessionRequest) -> Result<ResumeSessionResponse> {
                (**self).resume_session(a).await
            }
            async fn close_session(&self, a: CloseSessionRequest) -> Result<CloseSessionResponse> {
                (**self).close_session(a).await
            }
            async fn ext_method(&self, a: ExtRequest) -> Result<ExtResponse> {
                (**self).ext_method(a).await
            }
            async fn ext_notification(&self, a: ExtNotification) -> Result<()> {
                (**self).ext_notification(a).await
            }
        }
    };
}

macro_rules! forward_client {
    ($ptr:ident) => {
        #[async_trait::async_trait(?Send)]
        impl<T: Client + ?Sized> Client for $ptr<T> {
            async fn request_permission(&self, a: RequestPermissionRequest) -> Result<RequestPermissionResponse> {
                (**self).request_permission(a).await
            }
            async fn session_notification(&self, a: SessionNotification) -> Result<()> {
                (**self).session_notification(a).await
            }
            async fn write_text_file(&self, a: WriteTextFileRequest) -> Result<WriteTextFileResponse> {
                (**self).write_text_file(a).await
            }
            async fn read_text_file(&self, a: ReadTextFileRequest) -> Result<ReadTextFileResponse> {
                (**self).read_text_file(a).await
            }
            async fn create_terminal(&self, a: CreateTerminalRequest) -> Result<CreateTerminalResponse> {
                (**self).create_terminal(a).await
            }
            async fn terminal_output(&self, a: TerminalOutputRequest) -> Result<TerminalOutputResponse> {
                (**self).terminal_output(a).await
            }
            async fn release_terminal(&self, a: ReleaseTerminalRequest) -> Result<ReleaseTerminalResponse> {
                (**self).release_terminal(a).await
            }
            async fn wait_for_terminal_exit(
                &self,
                a: WaitForTerminalExitRequest,
            ) -> Result<WaitForTerminalExitResponse> {
                (**self).wait_for_terminal_exit(a).await
            }
            async fn kill_terminal(&self, a: KillTerminalRequest) -> Result<KillTerminalResponse> {
                (**self).kill_terminal(a).await
            }
            async fn ext_method(&self, a: ExtRequest) -> Result<ExtResponse> {
                (**self).ext_method(a).await
            }
            async fn ext_notification(&self, a: ExtNotification) -> Result<()> {
                (**self).ext_notification(a).await
            }
        }
    };
}

forward_agent!(Rc);
forward_agent!(Arc);
forward_client!(Rc);
forward_client!(Arc);
