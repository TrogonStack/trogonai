use serde::{Deserialize, Serialize};
use trogon_actor::{ActorContext, EntityActor};

/// Persisted state for a single pull request, accumulating across all events.
///
/// The runtime loads this before every event and saves it after — giving the
/// actor continuous memory of everything that happened since the PR opened.
#[derive(Default, Serialize, Deserialize, Clone, Debug)]
pub struct PrState {
    /// Total number of events processed for this PR.
    pub events_processed: u32,
    /// SHA of the most recently reviewed head commit, if known.
    pub last_reviewed_sha: Option<String>,
    /// Issues identified and tracked across all review sessions.
    pub issues_found: Vec<String>,
}

/// Entity Actor for the full lifecycle of a GitHub pull request.
///
/// Subscribes to `actors.pr.>` via [`trogon_actor::host::ActorHost`]. Entity
/// keys follow the pattern `{owner}.{repo}.{number}`, e.g.
/// `acme.myrepo.42` (NATS-safe dots instead of slashes).
///
/// Each event — PR opened, commit pushed, CI result, review requested — is
/// dispatched here by the router. The actor accumulates state so later events
/// have full context from earlier ones.
#[derive(Clone)]
pub struct PrActor;

impl EntityActor for PrActor {
    type State = PrState;
    type Error = std::convert::Infallible;

    fn actor_type() -> &'static str {
        "pr"
    }

    async fn handle(
        &mut self,
        state: &mut PrState,
        ctx: &ActorContext,
    ) -> Result<(), Self::Error> {
        state.events_processed += 1;

        tracing::info!(
            entity_key = %ctx.entity_key,
            events = state.events_processed,
            issues = state.issues_found.len(),
            "PR event received"
        );

        // Record the incoming event in the transcript.
        let user_msg = format!(
            "PR event #{n} for `{key}`.",
            n = state.events_processed,
            key = ctx.entity_key,
        );
        ctx.append_user_message(user_msg, None).await.ok();

        // Placeholder response — a real implementation calls the LLM here,
        // inspects the diff, posts a GitHub review comment, etc.
        let assistant_msg = format!(
            "Acknowledged event #{n}. State: {events} total event(s), {issues} tracked issue(s).",
            n = state.events_processed,
            events = state.events_processed,
            issues = state.issues_found.len(),
        );
        ctx.append_assistant_message(assistant_msg, None).await.ok();

        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_actor::ContextBuilder;

    #[tokio::test]
    async fn actor_type_is_pr() {
        assert_eq!(PrActor::actor_type(), "pr");
    }

    #[tokio::test]
    async fn handle_increments_events_processed() {
        let (ctx, _entries) = ContextBuilder::new("pr", "acme.repo.1").build();
        let mut state = PrState::default();
        let mut actor = PrActor;

        actor.handle(&mut state, &ctx).await.unwrap();
        assert_eq!(state.events_processed, 1);

        actor.handle(&mut state, &ctx).await.unwrap();
        assert_eq!(state.events_processed, 2);
    }

    #[tokio::test]
    async fn handle_appends_two_transcript_entries() {
        let (ctx, entries) = ContextBuilder::new("pr", "acme.repo.42").build();
        let mut state = PrState::default();
        PrActor.handle(&mut state, &ctx).await.unwrap();

        let snapshot = entries.lock().unwrap();
        assert_eq!(snapshot.len(), 2, "expected user + assistant entries");
    }

    #[tokio::test]
    async fn on_create_default_is_noop() {
        let mut state = PrState::default();
        PrActor::on_create(&mut state).await.unwrap();
        assert_eq!(state.events_processed, 0);
    }

    #[tokio::test]
    async fn state_default_is_zeroed() {
        let s = PrState::default();
        assert_eq!(s.events_processed, 0);
        assert!(s.last_reviewed_sha.is_none());
        assert!(s.issues_found.is_empty());
    }
}
