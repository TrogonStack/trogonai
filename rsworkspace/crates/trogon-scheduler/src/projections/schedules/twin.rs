    use buffa::MessageField;

    use crate::{projections_v1, v1};

    use projections_v1::__buffa::oneof::delivery::Kind as ViewDeliveryKind;
    use projections_v1::__buffa::oneof::delivery::nats_message::source::Kind as ViewSourceKind;
    use projections_v1::__buffa::oneof::schedule::Kind as ViewScheduleKind;
    use projections_v1::__buffa::oneof::schedule_status::Kind as ViewStatusKind;
    use v1::__buffa::oneof::delivery::Kind as EventDeliveryKind;
    use v1::__buffa::oneof::delivery::nats_message::source::Kind as EventSourceKind;
    use v1::__buffa::oneof::schedule::Kind as EventScheduleKind;
    use v1::__buffa::oneof::schedule_status::Kind as EventStatusKind;

    pub(super) fn status_to_projection(value: v1::ScheduleStatus) -> projections_v1::ScheduleStatus {
        projections_v1::ScheduleStatus {
            kind: value.kind.map(|kind| match kind {
                EventStatusKind::Scheduled(_) => {
                    ViewStatusKind::Scheduled(Box::new(projections_v1::schedule_status::Scheduled {}))
                }
                EventStatusKind::Paused(_) => {
                    ViewStatusKind::Paused(Box::new(projections_v1::schedule_status::Paused {}))
                }
            }),
        }
    }

    pub(super) fn schedule_to_projection(value: v1::Schedule) -> projections_v1::Schedule {
        projections_v1::Schedule {
            kind: value.kind.map(|kind| match kind {
                EventScheduleKind::At(at) => ViewScheduleKind::At(Box::new(projections_v1::schedule::At { at: at.at })),
                EventScheduleKind::Every(every) => {
                    ViewScheduleKind::Every(Box::new(projections_v1::schedule::Every { every: every.every }))
                }
                EventScheduleKind::Cron(cron) => ViewScheduleKind::Cron(Box::new(projections_v1::schedule::Cron {
                    expr: cron.expr,
                    timezone: cron.timezone,
                })),
                EventScheduleKind::Rrule(rrule) => ViewScheduleKind::Rrule(Box::new(projections_v1::schedule::RRule {
                    dtstart: rrule.dtstart,
                    rrule: rrule.rrule,
                    timezone: rrule.timezone,
                    rdate: rrule.rdate,
                    exdate: rrule.exdate,
                })),
            }),
        }
    }

    pub(super) fn delivery_to_projection(value: v1::Delivery) -> projections_v1::Delivery {
        projections_v1::Delivery {
            kind: value.kind.map(|kind| match kind {
                EventDeliveryKind::NatsMessage(nats) => {
                    ViewDeliveryKind::NatsMessage(Box::new(projections_v1::delivery::NatsMessage {
                        subject: nats.subject,
                        ttl: nats.ttl,
                        source: match nats.source.into_option() {
                            Some(source) => MessageField::some(projections_v1::delivery::nats_message::Source {
                                kind: source.kind.map(|kind| match kind {
                                    EventSourceKind::LatestFromSubject(latest) => ViewSourceKind::LatestFromSubject(
                                        Box::new(projections_v1::delivery::nats_message::LatestFromSubject {
                                            subject: latest.subject,
                                        }),
                                    ),
                                }),
                            }),
                            None => MessageField::none(),
                        },
                    }))
                }
            }),
        }
    }

    pub(super) fn message_to_projection(value: v1::Message) -> projections_v1::Message {
        projections_v1::Message {
            content: value.content,
            headers: value
                .headers
                .into_iter()
                .map(|header| projections_v1::Header {
                    name: header.name,
                    value: header.value,
                })
                .collect(),
        }
    }

