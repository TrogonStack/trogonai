pub use trogon_cron_jobs_proto::{
    JOB_ADDED_EVENT_TYPE, JOB_PAUSED_EVENT_TYPE, JOB_REMOVED_EVENT_TYPE, JOB_RESUMED_EVENT_TYPE, JobEventCodec,
    JobEventCodecError,
};

#[cfg(test)]
mod tests {
    use super::*;
    use protobuf::Parse as _;
    use trogon_cron_jobs_proto::v1;
    use trogon_eventsourcing::{EventCodec, EventData};

    #[test]
    fn event_data_and_recorded_event_helpers_work() {
        let mut removed = v1::JobEvent::new();
        removed.set_job_removed(v1::JobRemoved::new());
        let event = EventData::from_event("cleanup", &JobEventCodec, &removed).unwrap();
        assert_eq!(event.stream_id(), "cleanup");
        assert_eq!(event.event_type, JOB_REMOVED_EVENT_TYPE);
        assert!(v1::JobRemoved::parse(&event.payload).is_ok());
        assert_eq!(
            event.subject_with_prefix("cron.events.jobs."),
            "cron.events.jobs.cleanup"
        );

        assert_eq!(event.decode_data_with(&JobEventCodec).unwrap(), removed);

        let recorded = event.record(
            "cron.jobs.events.cleanup",
            None,
            Some(9),
            chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        );
        assert_eq!(recorded.stream_id(), "cleanup");
        assert_eq!(recorded.recorded_stream_id, "cron.jobs.events.cleanup");
        assert_eq!(recorded.log_position, Some(9));
        assert_eq!(
            recorded.subject_with_prefix("cron.events.jobs."),
            "cron.events.jobs.cleanup"
        );
        let mut expected = v1::JobEvent::new();
        expected.set_job_removed(v1::JobRemoved::new());
        assert_eq!(recorded.decode_data_with(&JobEventCodec).unwrap(), expected);
    }

    #[test]
    fn invalid_payload_fails_decode() {
        assert!(JobEventCodec.decode(JOB_REMOVED_EVENT_TYPE, "cleanup", b"\0").is_err());
        assert!(
            JobEventCodec
                .decode("trogon.cron.jobs.v1.Unknown", "cleanup", &[])
                .is_err()
        );
    }

    #[test]
    fn job_added_round_trips_through_contract() {
        let mut event = v1::JobEvent::new();
        let mut inner = v1::JobAdded::new();
        inner.set_job(job_details());
        event.set_job_added(inner);

        let encoded = JobEventCodec.encode(&event).unwrap();
        let decoded = JobEventCodec.decode(JOB_ADDED_EVENT_TYPE, "backup", &encoded).unwrap();

        assert_eq!(decoded, event);
        assert!(matches!(decoded.event(), v1::job_event::EventOneof::JobAdded(_)));
    }

    fn job_details() -> v1::JobDetails {
        let mut details = v1::JobDetails::new();
        details.set_status(v1::JobStatus::Enabled);
        details.set_schedule(cron_schedule());
        details.set_delivery(nats_delivery());
        details.set_message(message());
        details
    }

    fn cron_schedule() -> v1::JobSchedule {
        let mut schedule = v1::JobSchedule::new();
        let mut cron = v1::CronSchedule::new();
        cron.set_expr("0 * * * * *");
        cron.set_timezone("UTC");
        schedule.set_cron(cron);
        schedule
    }

    fn nats_delivery() -> v1::JobDelivery {
        let mut delivery = v1::JobDelivery::new();
        let mut nats = v1::NatsEventDelivery::new();
        nats.set_route("ops.backup");
        nats.set_ttl_sec(30);
        let mut source = v1::JobSamplingSource::new();
        let mut latest = v1::LatestFromSubjectSampling::new();
        latest.set_subject("events.backup");
        source.set_latest_from_subject(latest);
        nats.set_source(source);
        delivery.set_nats_event(nats);
        delivery
    }

    fn message() -> v1::JobMessage {
        let mut message = v1::JobMessage::new();
        message.set_content("hello");
        let mut header = v1::Header::new();
        header.set_name("x-kind");
        header.set_value("backup");
        message.headers_mut().push(header);
        message
    }
}
