    use super::*;
    use a2a::event::TaskStatusUpdateEvent;
    use a2a::types::TaskStatus;

    fn status_update(state: TaskState) -> StreamResponse {
        StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
            task_id: "task-1".into(),
            context_id: "ctx-1".into(),
            status: TaskStatus {
                state,
                message: None,
                timestamp: None,
            },
            metadata: None,
        })
    }

    #[test]
    fn idempotency_segment_distinguishes_every_terminal_variant() {
        assert_eq!(TerminalPushTaskState::Completed.idempotency_segment(), "completed");
        assert_eq!(TerminalPushTaskState::Failed.idempotency_segment(), "failed");
        assert_eq!(TerminalPushTaskState::Canceled.idempotency_segment(), "canceled");
        assert_eq!(TerminalPushTaskState::Rejected.idempotency_segment(), "rejected");
    }

    #[test]
    fn from_stream_terminal_response_maps_every_terminal_task_state() {
        assert_eq!(
            TerminalPushTaskState::from_stream_terminal_response(&status_update(TaskState::Completed)),
            Some(TerminalPushTaskState::Completed)
        );
        assert_eq!(
            TerminalPushTaskState::from_stream_terminal_response(&status_update(TaskState::Failed)),
            Some(TerminalPushTaskState::Failed)
        );
        assert_eq!(
            TerminalPushTaskState::from_stream_terminal_response(&status_update(TaskState::Canceled)),
            Some(TerminalPushTaskState::Canceled)
        );
        assert_eq!(
            TerminalPushTaskState::from_stream_terminal_response(&status_update(TaskState::Rejected)),
            Some(TerminalPushTaskState::Rejected)
        );
    }

    #[test]
    fn from_stream_terminal_response_returns_none_for_non_terminal_states() {
        assert_eq!(
            TerminalPushTaskState::from_stream_terminal_response(&status_update(TaskState::Working)),
            None
        );
        assert_eq!(
            TerminalPushTaskState::from_stream_terminal_response(&status_update(TaskState::Submitted)),
            None
        );
    }

    #[test]
    fn from_stream_terminal_response_returns_none_for_non_status_events() {
        let ev = StreamResponse::Message(a2a::types::Message {
            role: a2a::types::Role::Agent,
            parts: vec![],
            message_id: "m1".into(),
            metadata: None,
            task_id: None,
            context_id: None,
            extensions: None,
            reference_task_ids: None,
        });
        assert_eq!(TerminalPushTaskState::from_stream_terminal_response(&ev), None);
    }
