//! Live continuity-checkpoint transport: asks the **target** runner/model for a state
//! acknowledgement over NATS, so high-risk switches validate against the real destination
//! instead of echoing the Context Twin (cambio-modelo.md §10 Continuity Checkpoint:
//! "compile prompt for target model → ask target model for state acknowledgement").
//!
//! It opens an ephemeral session on the target prefix, sets the target model, prompts it
//! once with the framed acknowledgement request, collects the assistant text, then closes
//! the session. It is `Send + Sync` because it talks to the runner via `async_nats::Client`
//! (request-reply), not the `!Send` ACP `Bridge`.

use std::path::PathBuf;

use async_trait::async_trait;
use trogonai_switching::{AckTransport, SwitchingError};

use crate::session::{Session, StreamEvent, TrogonSession};

/// [`AckTransport`] backed by an ephemeral [`TrogonSession`] on the target runner.
pub struct SessionAckTransport {
    nats: async_nats::Client,
    target_prefix: String,
    target_model: String,
    cwd: PathBuf,
}

impl SessionAckTransport {
    pub fn new(nats: async_nats::Client, target_prefix: String, target_model: String, cwd: PathBuf) -> Self {
        Self {
            nats,
            target_prefix,
            target_model,
            cwd,
        }
    }

    fn fail(detail: impl std::fmt::Display) -> SwitchingError {
        SwitchingError::RunnerAcknowledgementFailed {
            detail: detail.to_string(),
        }
    }
}

#[async_trait]
impl AckTransport for SessionAckTransport {
    async fn ask(&self, prompt: &str) -> Result<String, SwitchingError> {
        // Ephemeral session on the target runner, pinned to the target model so the
        // acknowledgement comes from the model the switch is moving to.
        let session = TrogonSession::new(self.nats.clone(), &self.target_prefix, self.cwd.clone(), Vec::new())
            .await
            .map_err(Self::fail)?;
        if let Err(err) = session.set_model(&self.target_model).await {
            let _ = session.close().await;
            return Err(Self::fail(err));
        }

        let mut rx = match session.prompt(prompt).await {
            Ok(rx) => rx,
            Err(err) => {
                let _ = session.close().await;
                return Err(Self::fail(err));
            }
        };

        let mut text = String::new();
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::Text(chunk) => text.push_str(&chunk),
                StreamEvent::Error(message) => {
                    let _ = session.close().await;
                    return Err(Self::fail(message));
                }
                StreamEvent::Done(_) => break,
                _ => {}
            }
        }

        let _ = session.close().await;
        Ok(text)
    }
}
