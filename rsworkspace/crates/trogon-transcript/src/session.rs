use crate::{
    entry::{Role, TranscriptEntry, now_ms},
    error::TranscriptError,
    publisher::TranscriptPublisher,
    subject::transcript_subject,
};
use bytes::Bytes;
use uuid::Uuid;

/// An open transcript session for one actor invocation.
///
/// Each time an Entity Actor handles an event, it opens a new `Session`. Every
/// `append` call publishes one [`TranscriptEntry`] to JetStream on the subject:
///
/// ```text
/// transcripts.{actor_type}.{actor_key}.{session_id}
/// ```
///
/// The session_id is a random UUIDv4 generated at construction time, so
/// multiple sessions for the same entity are always distinguishable in the stream.
///
/// `Session<P>` is generic over `P: TranscriptPublisher` so it can be used with
/// the real NATS publisher in production and with `MockTranscriptPublisher` in
/// unit tests.
pub struct Session<P: TranscriptPublisher> {
    publisher: P,
    actor_type: String,
    actor_key: String,
    session_id: String,
}

impl<P: TranscriptPublisher> Session<P> {
    pub fn new(
        publisher: P,
        actor_type: impl Into<String>,
        actor_key: impl Into<String>,
    ) -> Self {
        Self {
            publisher,
            actor_type: actor_type.into(),
            actor_key: actor_key.into(),
            session_id: Uuid::new_v4().to_string(),
        }
    }

    /// The UUIDv4 that uniquely identifies this session within the entity's transcript.
    pub fn id(&self) -> &str {
        &self.session_id
    }

    pub fn actor_type(&self) -> &str {
        &self.actor_type
    }

    pub fn actor_key(&self) -> &str {
        &self.actor_key
    }

    /// Publish one entry to the transcript stream.
    pub async fn append(&self, entry: TranscriptEntry) -> Result<(), TranscriptError> {
        let subject = transcript_subject(&self.actor_type, &self.actor_key, &self.session_id);
        let payload = serde_json::to_vec(&entry).map_err(TranscriptError::Serialization)?;
        self.publisher
            .publish(subject, Bytes::from(payload))
            .await
    }

    // ── Convenience helpers ──────────────────────────────────────────────────

    pub async fn append_user_message(
        &self,
        content: impl Into<String>,
        tokens: Option<u32>,
    ) -> Result<(), TranscriptError> {
        self.append(TranscriptEntry::Message {
            role: Role::User,
            content: content.into(),
            timestamp: now_ms(),
            tokens,
        })
        .await
    }

    pub async fn append_assistant_message(
        &self,
        content: impl Into<String>,
        tokens: Option<u32>,
    ) -> Result<(), TranscriptError> {
        self.append(TranscriptEntry::Message {
            role: Role::Assistant,
            content: content.into(),
            timestamp: now_ms(),
            tokens,
        })
        .await
    }

    pub async fn append_system_message(
        &self,
        content: impl Into<String>,
    ) -> Result<(), TranscriptError> {
        self.append(TranscriptEntry::Message {
            role: Role::System,
            content: content.into(),
            timestamp: now_ms(),
            tokens: None,
        })
        .await
    }

    pub async fn append_tool_call(
        &self,
        name: impl Into<String>,
        input: serde_json::Value,
        output: serde_json::Value,
        duration_ms: u64,
    ) -> Result<(), TranscriptError> {
        self.append(TranscriptEntry::ToolCall {
            name: name.into(),
            input,
            output,
            duration_ms,
            timestamp: now_ms(),
        })
        .await
    }

    pub async fn append_routing_decision(
        &self,
        from: impl Into<String>,
        to: impl Into<String>,
        reasoning: impl Into<String>,
    ) -> Result<(), TranscriptError> {
        self.append(TranscriptEntry::RoutingDecision {
            from: from.into(),
            to: to.into(),
            reasoning: reasoning.into(),
            timestamp: now_ms(),
        })
        .await
    }

    pub async fn append_sub_agent_spawn(
        &self,
        parent: impl Into<String>,
        child: impl Into<String>,
        capability: impl Into<String>,
    ) -> Result<(), TranscriptError> {
        self.append(TranscriptEntry::SubAgentSpawn {
            parent: parent.into(),
            child: child.into(),
            capability: capability.into(),
            timestamp: now_ms(),
        })
        .await
    }
}

#[cfg(any(test, feature = "test-helpers"))]
pub mod tests_util {
    use super::*;
    use crate::publisher::mock::MockTranscriptPublisher;

    /// Creates a `Session` backed by a `MockTranscriptPublisher` for unit tests.
    pub fn mock_session(
        actor_type: impl Into<String>,
        actor_key: impl Into<String>,
    ) -> (Session<MockTranscriptPublisher>, MockTranscriptPublisher) {
        let publisher = MockTranscriptPublisher::new();
        let session = Session::new(publisher.clone(), actor_type, actor_key);
        (session, publisher)
    }
}

#[cfg(test)]
mod tests {
    use super::tests_util::mock_session;
    use crate::entry::TranscriptEntry;

    #[tokio::test]
    async fn append_serializes_and_publishes() {
        let (session, mock) = mock_session("pr", "owner/repo/1");
        session
            .append_user_message("Please review this PR.", None)
            .await
            .unwrap();

        let published = mock.take_published();
        assert_eq!(published.len(), 1);

        let (subject, payload) = &published[0];
        let expected_prefix = format!("transcripts.pr.owner.repo.1.{}", session.id());
        assert_eq!(subject, &expected_prefix);

        let entry: TranscriptEntry = serde_json::from_slice(payload).unwrap();
        match entry {
            TranscriptEntry::Message { content, .. } => {
                assert_eq!(content, "Please review this PR.");
            }
            other => panic!("unexpected entry variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn each_session_gets_unique_id() {
        let (s1, _) = mock_session("pr", "owner/repo/1");
        let (s2, _) = mock_session("pr", "owner/repo/1");
        assert_ne!(s1.id(), s2.id());
    }

    #[tokio::test]
    async fn routing_decision_round_trips() {
        let (session, mock) = mock_session("router", "global");
        session
            .append_routing_decision("gateway", "PrActor", "event is a pull_request")
            .await
            .unwrap();

        let published = mock.take_published();
        let (_, payload) = &published[0];
        let entry: TranscriptEntry = serde_json::from_slice(payload).unwrap();
        match entry {
            TranscriptEntry::RoutingDecision { from, to, reasoning, .. } => {
                assert_eq!(from, "gateway");
                assert_eq!(to, "PrActor");
                assert_eq!(reasoning, "event is a pull_request");
            }
            other => panic!("unexpected entry variant: {other:?}"),
        }
    }
}
