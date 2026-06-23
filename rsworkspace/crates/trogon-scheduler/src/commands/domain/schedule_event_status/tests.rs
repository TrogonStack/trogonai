use super::*;

    #[test]
    fn converts_paused_status_to_proto() {
        let status = v1::ScheduleStatus::from(ScheduleEventStatus::Paused);

        assert!(matches!(status.kind.unwrap(), v1::schedule_status::Kind::Paused(_)));
    }
