use trogonai_proto::scheduler::schedules::v1;

use super::SamplingSubject;

#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleEventSamplingSource {
    LatestFromSubject { subject: SamplingSubject },
}

impl From<&ScheduleEventSamplingSource> for v1::delivery::nats_message::Source {
    fn from(value: &ScheduleEventSamplingSource) -> Self {
        match value {
            ScheduleEventSamplingSource::LatestFromSubject { subject } => Self {
                kind: Some(
                    v1::delivery::nats_message::LatestFromSubject {
                        subject: subject.as_str().to_string(),
                    }
                    .into(),
                ),
            },
        }
    }
}

#[cfg(test)]
mod tests {
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
}
