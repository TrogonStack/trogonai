//! Handler routing and configuration tests

#[derive(Clone, Debug)]
pub struct NamedHandler {
    pub subject: String,
    pub handler_type: HandlerType,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HandlerType {
    Initialize,
    Authenticate,
    SessionPrompt,
}

impl NamedHandler {
    pub fn initialize_handler() -> Self {
        Self {
            name: "InitializeHandler".to_string(),
            subject: "acp.agent.initialize".to_string(),
            handler_type: HandlerType::Initialize,
        }
    }

    pub fn authenticate_handler() -> Self {
        Self {
            name: "AuthenticateHandler".to_string(),
            subject: "acp.agent.authenticate".to_string(),
            handler_type: HandlerType::Authenticate,
        }
    }

    pub fn session_prompt_handler(session_id: &str) -> Self {
        Self {
            name: format!("SessionPromptHandler[{}]", session_id),
            subject: format!("acp.{}.agent.session.prompt", session_id),
            handler_type: HandlerType::SessionPrompt,
        }
    }

    pub fn new(name: &str, subject: &str, handler_type: HandlerType) -> Self {
        Self {
            name: name.to_string(),
            subject: subject.to_string(),
            handler_type,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BridgeConfiguration {
    active_handlers: Vec<NamedHandler>,
}

impl BridgeConfiguration {
    pub fn standard_acp_bridge() -> Self {
        Self {
            active_handlers: vec![
                NamedHandler::initialize_handler(),
                NamedHandler::authenticate_handler(),
                NamedHandler::new(
                    "SessionNewHandler",
                    "acp.agent.session.new",
                    HandlerType::Authenticate,
                ),
            ],
        }
    }

    pub fn with_session_handler(mut self, session_id: &str) -> Self {
        self.active_handlers
            .push(NamedHandler::session_prompt_handler(session_id));
        self
    }

    pub fn find_handler_for_subject(&self, subject: &str) -> Option<NamedHandler> {
        self.active_handlers
            .iter()
            .find(|h| h.subject == subject)
            .cloned()
    }
}
