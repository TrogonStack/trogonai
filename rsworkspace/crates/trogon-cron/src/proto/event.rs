pub use trogon_cron_jobs_proto::{
    JOB_ADDED_EVENT_TYPE, JOB_PAUSED_EVENT_TYPE, JOB_REMOVED_EVENT_TYPE, JOB_RESUMED_EVENT_TYPE, JobEventCodec,
    JobEventCodecError,
};

#[cfg(test)]
mod tests {
    use super::*;
    use buffa::{Message as _, MessageField};
    use trogon_cron_jobs_proto::v1;
    use trogon_eventsourcing::{EventCodec, EventData};

    #[test]
    fn event_data_and_recorded_event_helpers_work() {
        let removed = v1::JobEvent {
            event: Some(v1::JobRemoved {}.into()),
        };
        let event = EventData::from_event("cleanup", &JobEventCodec, &removed).unwrap();
        assert_eq!(event.stream_id(), "cleanup");
        assert_eq!(event.event_type, JOB_REMOVED_EVENT_TYPE);
        assert!(v1::JobRemoved::decode_from_slice(&event.payload).is_ok());
        assert_eq!(
            event.subject_with_prefix("cron.jobs.events."),
            "cron.jobs.events.cleanup"
        );

        assert_eq!(event.decode_data_with(&JobEventCodec).unwrap(), removed);

        let recorded = event.record(
            None,
            chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        );
        assert_eq!(recorded.stream_id(), "cleanup");
        assert_eq!(
            recorded.subject_with_prefix("cron.jobs.events."),
            "cron.jobs.events.cleanup"
        );
        let expected = v1::JobEvent {
            event: Some(v1::JobRemoved {}.into()),
        };
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
        let event = v1::JobEvent {
            event: Some(
                v1::JobAdded {
                    job: MessageField::some(job_details()),
                }
                .into(),
            ),
        };

        let encoded = JobEventCodec.encode(&event).unwrap();
        let decoded = JobEventCodec.decode(JOB_ADDED_EVENT_TYPE, "backup", &encoded).unwrap();

        assert_eq!(decoded, event);
        assert!(matches!(
            decoded.event,
            Some(v1::__buffa::oneof::job_event::Event::JobAdded(_))
        ));
    }

    fn job_details() -> v1::JobDetails {
        v1::JobDetails {
            status: v1::JobStatus::JOB_STATUS_ENABLED,
            schedule: MessageField::some(cron_schedule()),
            delivery: MessageField::some(nats_delivery()),
            message: MessageField::some(message()),
        }
    }

    fn cron_schedule() -> v1::JobSchedule {
        v1::JobSchedule {
            kind: Some(
                v1::CronSchedule {
                    expr: "0 * * * * *".to_string(),
                    timezone: "UTC".to_string(),
                }
                .into(),
            ),
        }
    }

    fn nats_delivery() -> v1::JobDelivery {
        v1::JobDelivery {
            kind: Some(
                v1::NatsEventDelivery {
                    route: "ops.backup".to_string(),
                    ttl_sec: Some(30),
                    source: MessageField::some(v1::JobSamplingSource {
                        kind: Some(
                            v1::LatestFromSubjectSampling {
                                subject: "events.backup".to_string(),
                            }
                            .into(),
                        ),
                    }),
                }
                .into(),
            ),
        }
    }

    fn message() -> v1::JobMessage {
        v1::JobMessage {
            content: "hello".to_string(),
            headers: vec![v1::Header {
                name: "x-kind".to_string(),
                value: "backup".to_string(),
            }],
        }
    }
}
