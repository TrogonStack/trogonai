//! Field-by-field moves between the event (`v1`) and checkpoint
//! (`checkpoints_v1`) proto twins. The twins share every nested well-known
//! type, so only the outer wrappers need mapping; this avoids the
//! encode-then-decode pass a wire transcode would add to every checkpoint
//! read and write.

use trogonai_proto::scheduler::schedules::{checkpoints_v1, v1};

use checkpoints_v1::__buffa::oneof::delivery::Kind as CheckpointDeliveryKind;
use checkpoints_v1::__buffa::oneof::delivery::nats_message::source::Kind as CheckpointSourceKind;
use checkpoints_v1::__buffa::oneof::schedule::Kind as CheckpointScheduleKind;
use v1::__buffa::oneof::delivery::Kind as EventDeliveryKind;
use v1::__buffa::oneof::delivery::nats_message::source::Kind as EventSourceKind;
use v1::__buffa::oneof::schedule::Kind as EventScheduleKind;

pub fn schedule_to_checkpoint(value: v1::Schedule) -> checkpoints_v1::Schedule {
    checkpoints_v1::Schedule {
        kind: value.kind.map(|kind| match kind {
            EventScheduleKind::At(at) => {
                CheckpointScheduleKind::At(Box::new(checkpoints_v1::schedule::At { at: at.at }))
            }
            EventScheduleKind::Every(every) => {
                CheckpointScheduleKind::Every(Box::new(checkpoints_v1::schedule::Every { every: every.every }))
            }
            EventScheduleKind::Cron(cron) => {
                CheckpointScheduleKind::Cron(Box::new(checkpoints_v1::schedule::Cron {
                    expr: cron.expr,
                    timezone: cron.timezone,
                }))
            }
            EventScheduleKind::Rrule(rrule) => {
                CheckpointScheduleKind::Rrule(Box::new(checkpoints_v1::schedule::RRule {
                    dtstart: rrule.dtstart,
                    rrule: rrule.rrule,
                    timezone: rrule.timezone,
                    rdate: rrule.rdate,
                    exdate: rrule.exdate,
                }))
            }
        }),
    }
}

pub fn schedule_from_checkpoint(value: checkpoints_v1::Schedule) -> v1::Schedule {
    v1::Schedule {
        kind: value.kind.map(|kind| match kind {
            CheckpointScheduleKind::At(at) => EventScheduleKind::At(Box::new(v1::schedule::At { at: at.at })),
            CheckpointScheduleKind::Every(every) => {
                EventScheduleKind::Every(Box::new(v1::schedule::Every { every: every.every }))
            }
            CheckpointScheduleKind::Cron(cron) => EventScheduleKind::Cron(Box::new(v1::schedule::Cron {
                expr: cron.expr,
                timezone: cron.timezone,
            })),
            CheckpointScheduleKind::Rrule(rrule) => EventScheduleKind::Rrule(Box::new(v1::schedule::RRule {
                dtstart: rrule.dtstart,
                rrule: rrule.rrule,
                timezone: rrule.timezone,
                rdate: rrule.rdate,
                exdate: rrule.exdate,
            })),
        }),
    }
}

pub fn delivery_to_checkpoint(value: v1::Delivery) -> checkpoints_v1::Delivery {
    checkpoints_v1::Delivery {
        kind: value.kind.map(|kind| match kind {
            EventDeliveryKind::NatsMessage(nats) => {
                CheckpointDeliveryKind::NatsMessage(Box::new(checkpoints_v1::delivery::NatsMessage {
                    subject: nats.subject,
                    ttl: nats.ttl,
                    source: match nats.source.into_option() {
                        Some(source) => buffa::MessageField::some(checkpoints_v1::delivery::nats_message::Source {
                            kind: source.kind.map(|kind| match kind {
                                EventSourceKind::LatestFromSubject(latest) => {
                                    CheckpointSourceKind::LatestFromSubject(Box::new(
                                        checkpoints_v1::delivery::nats_message::LatestFromSubject {
                                            subject: latest.subject,
                                        },
                                    ))
                                }
                            }),
                        }),
                        None => buffa::MessageField::none(),
                    },
                }))
            }
        }),
    }
}

pub fn delivery_from_checkpoint(value: checkpoints_v1::Delivery) -> v1::Delivery {
    v1::Delivery {
        kind: value.kind.map(|kind| match kind {
            CheckpointDeliveryKind::NatsMessage(nats) => {
                EventDeliveryKind::NatsMessage(Box::new(v1::delivery::NatsMessage {
                    subject: nats.subject,
                    ttl: nats.ttl,
                    source: match nats.source.into_option() {
                        Some(source) => buffa::MessageField::some(v1::delivery::nats_message::Source {
                            kind: source.kind.map(|kind| match kind {
                                CheckpointSourceKind::LatestFromSubject(latest) => {
                                    EventSourceKind::LatestFromSubject(Box::new(
                                        v1::delivery::nats_message::LatestFromSubject {
                                            subject: latest.subject,
                                        },
                                    ))
                                }
                            }),
                        }),
                        None => buffa::MessageField::none(),
                    },
                }))
            }
        }),
    }
}

pub fn message_to_checkpoint(value: v1::Message) -> checkpoints_v1::Message {
    checkpoints_v1::Message {
        content: value.content,
        headers: value
            .headers
            .into_iter()
            .map(|header| checkpoints_v1::Header {
                name: header.name,
                value: header.value,
            })
            .collect(),
    }
}

pub fn message_from_checkpoint(value: checkpoints_v1::Message) -> v1::Message {
    v1::Message {
        content: value.content,
        headers: value
            .headers
            .into_iter()
            .map(|header| v1::Header {
                name: header.name,
                value: header.value,
            })
            .collect(),
    }
}
