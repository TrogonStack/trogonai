use super::*;
    use crate::commands::domain::SamplingSubject;

    #[test]
    fn converts_latest_from_subject_to_proto_source() {
        let source = ScheduleEventSamplingSource::LatestFromSubject {
            subject: SamplingSubject::new("agent.events").unwrap(),
        };
        let proto = v1::delivery::nats_message::Source::from(&source);

        match proto.kind.unwrap() {
            v1::delivery::nats_message::source::Kind::LatestFromSubject(inner) => {
                assert_eq!(inner.subject, "agent.events");
            }
        }
    }
