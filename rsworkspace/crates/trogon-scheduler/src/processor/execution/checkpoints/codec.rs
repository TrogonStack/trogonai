//! Encoding of [`ScheduleCheckpointRecord`] for the NATS KV checkpoint bucket.
//!
//! The checkpoint bucket is a rebuildable cache, not the source of truth, so its
//! wire format is deliberately decoupled from the command event stream. Checkpoints
//! are persisted as a purpose-built protobuf snapshot rather than event payloads
//! or a JSON envelope.

use buffa::Message as _;
use trogon_decider_runtime::StreamPosition;
use trogonai_proto::scheduler::schedules::{checkpoints_v1, v1};

use crate::commands::domain::{MessageEnvelope, ScheduleEventDelivery, ScheduleEventSchedule};
use crate::processor::execution::reconciliation::{
    ScheduleEventDecodeError, delivery_from_proto, message_from_proto, schedule_from_proto, schedule_id_from,
};

use super::{ReconcileOutcome, ScheduleCheckpointRecord, ScheduleStatus};

/// Error raised while encoding or decoding a stored schedule checkpoint record.
#[derive(Debug, thiserror::Error)]
pub enum CheckpointCodecError {
    /// The stored JSON envelope could not be parsed.
    #[error("checkpoint record JSON is invalid: {source}")]
    Json {
        #[source]
        source: serde_json::Error,
    },
    /// The stored protobuf snapshot could not be decoded.
    #[error("checkpoint record wire format is invalid: {source}")]
    Wire {
        #[source]
        source: buffa::DecodeError,
    },
    /// The decoded snapshot could not be rebuilt into domain value objects.
    #[error("checkpoint record snapshot could not be rebuilt: {source}")]
    Domain {
        #[source]
        source: ScheduleEventDecodeError,
    },
    /// The stored stream position was zero, which is never valid.
    #[error("checkpoint stream position must be greater than zero")]
    StreamPosition,
    /// The forward snapshot conversion to proto failed.
    #[error("checkpoint snapshot could not be encoded to proto")]
    SnapshotConversion,
    /// Envelope metadata could not be scanned from corrupt checkpoint bytes.
    #[error("checkpoint envelope metadata is invalid")]
    Envelope,
}

/// Metadata parsed from a corrupt checkpoint protobuf envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorruptCheckpointEnvelope {
    /// Stream watermark from the envelope, when it parses.
    pub watermark: Option<StreamPosition>,
    /// Last applied event id from the envelope, when present.
    pub last_applied_event_id: Option<String>,
}

/// Reads envelope metadata from checkpoint bytes via a raw protobuf field scan,
/// even when nested snapshot fields are corrupt.
///
/// Each field is recovered independently: a missing or invalid watermark must
/// not discard a parseable event id (and vice versa), because the duplicate
/// fast-path can act on either field alone.
pub fn decode_checkpoint_envelope(bytes: &[u8]) -> CorruptCheckpointEnvelope {
    let mut watermark = None;
    let mut last_applied_event_id = None;

    // Best-effort: the scan stops at the first malformed field, but the
    // envelope fields are encoded before the nested snapshots, so anything
    // parsed before a truncation point is kept and the idempotency guard
    // still works on a partially written blob.
    let _ = scan_protobuf_fields(bytes, |field_number, wire_type, payload| {
        match (field_number, wire_type) {
            (3, WireType::Varint) => watermark = decode_varint(payload),
            (4, WireType::LengthDelimited) => {
                if let Ok(event_id) = String::from_utf8(payload.to_vec()) {
                    last_applied_event_id = Some(event_id);
                }
            }
            _ => {}
        }
    });

    CorruptCheckpointEnvelope {
        watermark: watermark.and_then(|watermark| StreamPosition::try_new(watermark).ok()),
        last_applied_event_id,
    }
}

/// Encodes a [`ScheduleCheckpointRecord`] into the bytes stored under its KV key.
pub fn encode_checkpoint_record(record: &ScheduleCheckpointRecord) -> Result<Vec<u8>, CheckpointCodecError> {
    let schedule = v1::Schedule::try_from(&ScheduleEventSchedule::from(&record.schedule))
        .map_err(|_| CheckpointCodecError::SnapshotConversion)?;
    let delivery = v1::Delivery::try_from(&ScheduleEventDelivery::from(&record.delivery))
        .map_err(|_| CheckpointCodecError::SnapshotConversion)?;
    let message = v1::Message::from(&MessageEnvelope::from(&record.message));

    let stored = checkpoints_v1::ScheduleCheckpoint {
        schedule_id: Some(record.schedule_id.as_str().to_string()),
        status: Some(checkpoint_status_to_proto(record.status).into()),
        last_applied_stream_position: Some(record.last_applied_stream_position.as_u64()),
        last_applied_event_id: record.last_applied_event_id.clone(),
        last_outcome: Some(checkpoint_outcome_to_proto(record.last_outcome).into()),
        schedule: buffa::MessageField::some(twin::schedule_to_checkpoint(schedule)),
        delivery: buffa::MessageField::some(twin::delivery_to_checkpoint(delivery)),
        message: buffa::MessageField::some(twin::message_to_checkpoint(message)),
    };

    Ok(stored.encode_to_vec())
}

/// Decodes the bytes stored under a KV key back into a [`ScheduleCheckpointRecord`].
pub fn decode_checkpoint_record(bytes: &[u8]) -> Result<ScheduleCheckpointRecord, CheckpointCodecError> {
    let stored = checkpoints_v1::ScheduleCheckpoint::decode_from_slice(bytes)
        .map_err(|source| CheckpointCodecError::Wire { source })?;

    let schedule_id = schedule_id_from(stored.schedule_id.as_deref().ok_or(CheckpointCodecError::Domain {
        source: ScheduleEventDecodeError::MissingField { field: "schedule_id" },
    })?)
    .map_err(|source| CheckpointCodecError::Domain { source })?;

    let schedule = schedule_from_proto(&twin::schedule_from_checkpoint(stored.schedule.into_option().ok_or(
        CheckpointCodecError::Domain {
            source: ScheduleEventDecodeError::MissingField { field: "schedule" },
        },
    )?))
    .map_err(|source| CheckpointCodecError::Domain { source })?;
    let delivery = delivery_from_proto(&twin::delivery_from_checkpoint(stored.delivery.into_option().ok_or(
        CheckpointCodecError::Domain {
            source: ScheduleEventDecodeError::MissingField { field: "delivery" },
        },
    )?))
    .map_err(|source| CheckpointCodecError::Domain { source })?;
    let message = message_from_proto(&twin::message_from_checkpoint(stored.message.into_option().ok_or(
        CheckpointCodecError::Domain {
            source: ScheduleEventDecodeError::MissingField { field: "message" },
        },
    )?))
    .map_err(|source| CheckpointCodecError::Domain { source })?;

    let last_applied_stream_position = StreamPosition::try_new(stored.last_applied_stream_position.ok_or(
        CheckpointCodecError::Domain {
            source: ScheduleEventDecodeError::MissingField {
                field: "last_applied_stream_position",
            },
        },
    )?)
    .map_err(|_| CheckpointCodecError::StreamPosition)?;

    Ok(ScheduleCheckpointRecord {
        schedule_id,
        status: checkpoint_status_from_proto(stored.status.as_ref())?,
        schedule,
        delivery,
        message,
        last_applied_stream_position,
        last_applied_event_id: stored.last_applied_event_id,
        last_outcome: checkpoint_outcome_from_proto(stored.last_outcome.as_ref())?,
    })
}

/// Replaces the schedule snapshot field with invalid bytes while preserving the
/// rest of the checkpoint envelope.
#[cfg(test)]
pub(crate) fn corrupt_checkpoint_schedule(bytes: &[u8]) -> Vec<u8> {
    rewrite_length_delimited_field(bytes, 6, &[0xff, 0xff])
}

/// Rewrites the checkpoint event id field without touching the rest of the snapshot.
#[cfg(test)]
pub(crate) fn corrupt_checkpoint_event_id(bytes: &[u8]) -> Vec<u8> {
    rewrite_length_delimited_field(bytes, 4, &[0xff, 0xfe])
}

/// Rewrites the checkpoint stream watermark without touching the rest of the snapshot.
#[cfg(test)]
pub(crate) fn rewrite_checkpoint_watermark(bytes: &[u8], position: u64) -> Vec<u8> {
    rewrite_varint_field(bytes, 3, position)
}

fn checkpoint_status_to_proto(status: ScheduleStatus) -> checkpoints_v1::ScheduleCheckpointStatus {
    match status {
        ScheduleStatus::Scheduled => checkpoints_v1::ScheduleCheckpointStatus::Scheduled,
        ScheduleStatus::Paused => checkpoints_v1::ScheduleCheckpointStatus::Paused,
        ScheduleStatus::Removed => checkpoints_v1::ScheduleCheckpointStatus::Removed,
        ScheduleStatus::Unsupported => checkpoints_v1::ScheduleCheckpointStatus::Unsupported,
        ScheduleStatus::Expired => checkpoints_v1::ScheduleCheckpointStatus::Expired,
        // Never written by this version: every saved checkpoint carries a
        // status freshly assigned by reconciliation.
        ScheduleStatus::Unknown => checkpoints_v1::ScheduleCheckpointStatus::Unspecified,
    }
}

fn checkpoint_status_from_proto(
    status: Option<&buffa::EnumValue<checkpoints_v1::ScheduleCheckpointStatus>>,
) -> Result<ScheduleStatus, CheckpointCodecError> {
    let status = status.ok_or(CheckpointCodecError::Domain {
        source: ScheduleEventDecodeError::MissingField { field: "status" },
    })?;
    match status.as_known() {
        Some(checkpoints_v1::ScheduleCheckpointStatus::Scheduled) => Ok(ScheduleStatus::Scheduled),
        Some(checkpoints_v1::ScheduleCheckpointStatus::Paused) => Ok(ScheduleStatus::Paused),
        Some(checkpoints_v1::ScheduleCheckpointStatus::Removed) => Ok(ScheduleStatus::Removed),
        Some(checkpoints_v1::ScheduleCheckpointStatus::Unsupported) => Ok(ScheduleStatus::Unsupported),
        Some(checkpoints_v1::ScheduleCheckpointStatus::Expired) => Ok(ScheduleStatus::Expired),
        Some(checkpoints_v1::ScheduleCheckpointStatus::Unspecified) => Err(CheckpointCodecError::Domain {
            source: ScheduleEventDecodeError::MissingField { field: "status" },
        }),
        // A value added by a newer deployment: keep the record readable so a
        // rolling deploy cannot route this schedule into the corrupt path.
        None => Ok(ScheduleStatus::Unknown),
    }
}

fn checkpoint_outcome_to_proto(outcome: ReconcileOutcome) -> checkpoints_v1::ReconcileOutcome {
    match outcome {
        ReconcileOutcome::Published => checkpoints_v1::ReconcileOutcome::Published,
        ReconcileOutcome::Purged => checkpoints_v1::ReconcileOutcome::Purged,
        ReconcileOutcome::StoredPaused => checkpoints_v1::ReconcileOutcome::StoredPaused,
        ReconcileOutcome::Unsupported => checkpoints_v1::ReconcileOutcome::Unsupported,
        ReconcileOutcome::Expired => checkpoints_v1::ReconcileOutcome::Expired,
        ReconcileOutcome::DuplicateStale => checkpoints_v1::ReconcileOutcome::DuplicateStale,
        // Never written by this version: every saved checkpoint carries an
        // outcome freshly assigned by reconciliation.
        ReconcileOutcome::Unknown => checkpoints_v1::ReconcileOutcome::Unspecified,
    }
}

fn checkpoint_outcome_from_proto(
    outcome: Option<&buffa::EnumValue<checkpoints_v1::ReconcileOutcome>>,
) -> Result<ReconcileOutcome, CheckpointCodecError> {
    let outcome = outcome.ok_or(CheckpointCodecError::Domain {
        source: ScheduleEventDecodeError::MissingField { field: "last_outcome" },
    })?;
    match outcome.as_known() {
        Some(checkpoints_v1::ReconcileOutcome::Published) => Ok(ReconcileOutcome::Published),
        Some(checkpoints_v1::ReconcileOutcome::Purged) => Ok(ReconcileOutcome::Purged),
        Some(checkpoints_v1::ReconcileOutcome::StoredPaused) => Ok(ReconcileOutcome::StoredPaused),
        Some(checkpoints_v1::ReconcileOutcome::Unsupported) => Ok(ReconcileOutcome::Unsupported),
        Some(checkpoints_v1::ReconcileOutcome::Expired) => Ok(ReconcileOutcome::Expired),
        Some(checkpoints_v1::ReconcileOutcome::DuplicateStale) => Ok(ReconcileOutcome::DuplicateStale),
        Some(checkpoints_v1::ReconcileOutcome::Unspecified) => Err(CheckpointCodecError::Domain {
            source: ScheduleEventDecodeError::MissingField { field: "last_outcome" },
        }),
        // A value added by a newer deployment: keep the record readable so a
        // rolling deploy cannot route this schedule into the corrupt path.
        None => Ok(ReconcileOutcome::Unknown),
    }
}

/// Field-by-field moves between the event (`v1`) and checkpoint
/// (`checkpoints_v1`) proto twins. The twins share every nested well-known
/// type, so only the outer wrappers need mapping; this avoids the
/// encode-then-decode pass a wire transcode would add to every checkpoint
/// read and write.
#[allow(inline_module_block)]
mod twin {
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireType {
    Varint,
    Fixed64,
    LengthDelimited,
    StartGroup,
    EndGroup,
    Fixed32,
}

impl WireType {
    fn from_tag(tag: u64) -> Option<Self> {
        match tag & 0x7 {
            0 => Some(Self::Varint),
            1 => Some(Self::Fixed64),
            2 => Some(Self::LengthDelimited),
            3 => Some(Self::StartGroup),
            4 => Some(Self::EndGroup),
            5 => Some(Self::Fixed32),
            _ => None,
        }
    }
}

fn scan_protobuf_fields(bytes: &[u8], mut visit: impl FnMut(u32, WireType, &[u8])) -> Result<(), CheckpointCodecError> {
    let mut offset = 0;
    while offset < bytes.len() {
        let (tag, next) = read_varint(bytes, offset).ok_or(CheckpointCodecError::Envelope)?;
        offset = next;
        let field_number = (tag >> 3) as u32;
        let wire_type = WireType::from_tag(tag).ok_or(CheckpointCodecError::Envelope)?;

        match wire_type {
            WireType::Varint => {
                let value_start = offset;
                let (_, next) = read_varint(bytes, offset).ok_or(CheckpointCodecError::Envelope)?;
                visit(field_number, wire_type, &bytes[value_start..next]);
                offset = next;
            }
            WireType::LengthDelimited => {
                let (length, next) = read_varint(bytes, offset).ok_or(CheckpointCodecError::Envelope)?;
                offset = next;
                let end = offset
                    .checked_add(length as usize)
                    .ok_or(CheckpointCodecError::Envelope)?;
                if end > bytes.len() {
                    return Err(CheckpointCodecError::Envelope);
                }
                visit(field_number, wire_type, &bytes[offset..end]);
                offset = end;
            }
            WireType::Fixed64 => {
                offset = offset.checked_add(8).ok_or(CheckpointCodecError::Envelope)?;
                if offset > bytes.len() {
                    return Err(CheckpointCodecError::Envelope);
                }
            }
            WireType::Fixed32 => {
                offset = offset.checked_add(4).ok_or(CheckpointCodecError::Envelope)?;
                if offset > bytes.len() {
                    return Err(CheckpointCodecError::Envelope);
                }
            }
            WireType::StartGroup | WireType::EndGroup => return Err(CheckpointCodecError::Envelope),
        }
    }

    Ok(())
}

#[cfg(test)]
fn rewrite_varint_field(bytes: &[u8], field_number: u32, value: u64) -> Vec<u8> {
    let mut rewritten = Vec::new();
    let mut offset = 0;
    let mut replaced = false;

    while offset < bytes.len() {
        let field_start = offset;
        let Some((tag, next)) = read_varint(bytes, offset) else {
            rewritten.extend_from_slice(&bytes[field_start..]);
            break;
        };
        offset = next;
        let current_field = (tag >> 3) as u32;
        let Some(wire_type) = WireType::from_tag(tag) else {
            rewritten.extend_from_slice(&bytes[field_start..]);
            break;
        };

        match wire_type {
            WireType::Varint => {
                let Some((_, next)) = read_varint(bytes, offset) else {
                    rewritten.extend_from_slice(&bytes[field_start..]);
                    break;
                };
                offset = next;
                if current_field == field_number {
                    rewritten.extend_from_slice(&encode_varint((field_number as u64) << 3));
                    rewritten.extend_from_slice(&encode_varint(value));
                    replaced = true;
                } else {
                    rewritten.extend_from_slice(&bytes[field_start..offset]);
                }
            }
            WireType::LengthDelimited => {
                let Some((length, next)) = read_varint(bytes, offset) else {
                    rewritten.extend_from_slice(&bytes[field_start..]);
                    break;
                };
                offset = next;
                let end = offset.saturating_add(length as usize);
                if end > bytes.len() {
                    rewritten.extend_from_slice(&bytes[field_start..]);
                    break;
                }
                offset = end;
                rewritten.extend_from_slice(&bytes[field_start..offset]);
            }
            WireType::Fixed64 => offset = offset.saturating_add(8).min(bytes.len()),
            WireType::Fixed32 => offset = offset.saturating_add(4).min(bytes.len()),
            WireType::StartGroup | WireType::EndGroup => {
                rewritten.extend_from_slice(&bytes[field_start..]);
                break;
            }
        }
    }

    if !replaced {
        rewritten.extend_from_slice(&encode_varint((field_number as u64) << 3));
        rewritten.extend_from_slice(&encode_varint(value));
    }

    rewritten
}

#[cfg(test)]
fn rewrite_length_delimited_field(bytes: &[u8], field_number: u32, replacement: &[u8]) -> Vec<u8> {
    let mut rewritten = Vec::new();
    let mut offset = 0;
    let mut replaced = false;

    while offset < bytes.len() {
        let field_start = offset;
        let Some((tag, next)) = read_varint(bytes, offset) else {
            rewritten.extend_from_slice(&bytes[field_start..]);
            break;
        };
        offset = next;
        let current_field = (tag >> 3) as u32;
        let Some(wire_type) = WireType::from_tag(tag) else {
            rewritten.extend_from_slice(&bytes[field_start..]);
            break;
        };

        match wire_type {
            WireType::Varint => {
                let Some((_, next)) = read_varint(bytes, offset) else {
                    rewritten.extend_from_slice(&bytes[field_start..]);
                    break;
                };
                offset = next;
                rewritten.extend_from_slice(&bytes[field_start..offset]);
            }
            WireType::LengthDelimited => {
                let Some((length, next)) = read_varint(bytes, offset) else {
                    rewritten.extend_from_slice(&bytes[field_start..]);
                    break;
                };
                offset = next;
                let end = offset.saturating_add(length as usize);
                if end > bytes.len() {
                    rewritten.extend_from_slice(&bytes[field_start..]);
                    break;
                }
                if current_field == field_number {
                    rewritten.extend_from_slice(&encode_varint((field_number as u64) << 3 | 2));
                    rewritten.extend_from_slice(&encode_varint(replacement.len() as u64));
                    rewritten.extend_from_slice(replacement);
                    replaced = true;
                } else {
                    rewritten.extend_from_slice(&bytes[field_start..end]);
                }
                offset = end;
            }
            WireType::Fixed64 => offset = offset.saturating_add(8).min(bytes.len()),
            WireType::Fixed32 => offset = offset.saturating_add(4).min(bytes.len()),
            WireType::StartGroup | WireType::EndGroup => {
                rewritten.extend_from_slice(&bytes[field_start..]);
                break;
            }
        }
    }

    if !replaced {
        rewritten.extend_from_slice(&encode_varint((field_number as u64) << 3 | 2));
        rewritten.extend_from_slice(&encode_varint(replacement.len() as u64));
        rewritten.extend_from_slice(replacement);
    }

    rewritten
}

fn read_varint(bytes: &[u8], mut offset: usize) -> Option<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0;
    while offset < bytes.len() {
        let byte = bytes[offset];
        offset += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some((value, offset));
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
    None
}

fn decode_varint(payload: &[u8]) -> Option<u64> {
    let mut value = 0u64;
    for (index, byte) in payload.iter().copied().enumerate() {
        let shift = index * 7;
        if shift >= 64 {
            return None;
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
fn encode_varint(mut value: u64) -> Vec<u8> {
    let mut encoded = Vec::new();
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        encoded.push(byte);
        if value == 0 {
            break;
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{DateTime, Utc};
    use trogonai_proto::scheduler::schedules::checkpoints_v1;

    use super::*;
    use crate::commands::domain::{Delivery, MessageContent, Schedule, ScheduleHeaders, ScheduleMessage};

    fn record(schedule: Schedule, status: ScheduleStatus, outcome: ReconcileOutcome) -> ScheduleCheckpointRecord {
        let schedule_id = crate::commands::domain::ScheduleId::parse("orders/created").unwrap();
        ScheduleCheckpointRecord {
            schedule_id,
            status,
            schedule,
            delivery: Delivery::NatsEvent {
                route: crate::commands::domain::DeliveryRoute::new("agent.run").unwrap(),
                ttl: Some(crate::commands::domain::TtlDuration::from_secs(45).unwrap()),
                source: Some(crate::commands::domain::SamplingSource::latest_from_subject("agent.events").unwrap()),
            },
            message: ScheduleMessage {
                content: MessageContent::json(r#"{"ok":true}"#),
                headers: ScheduleHeaders::new([("x-kind", "heartbeat")]).unwrap(),
            },
            last_applied_stream_position: StreamPosition::try_new(7).unwrap(),
            last_applied_event_id: Some("event-7".to_string()),
            last_outcome: outcome,
        }
    }

    fn at() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2030-01-02T03:04:05Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn checkpoint_record_round_trips_through_the_codec() {
        let original = record(
            Schedule::cron("0 0 * * * *", Some("America/New_York".to_string())).unwrap(),
            ScheduleStatus::Scheduled,
            ReconcileOutcome::Published,
        );

        let encoded = encode_checkpoint_record(&original).unwrap();
        let decoded = decode_checkpoint_record(&encoded).unwrap();

        assert_eq!(decoded, original);
    }

    #[test]
    fn round_trips_every_schedule_kind_and_status() {
        let cases = [
            (
                Schedule::At { at: at() },
                ScheduleStatus::Scheduled,
                ReconcileOutcome::Published,
            ),
            (
                Schedule::every(Duration::from_secs(30)).unwrap(),
                ScheduleStatus::Paused,
                ReconcileOutcome::Purged,
            ),
            (
                Schedule::rrule("2026-01-01T00:00:00Z", "FREQ=DAILY;COUNT=2", None).unwrap(),
                ScheduleStatus::Unsupported,
                ReconcileOutcome::Unsupported,
            ),
            (
                Schedule::every(Duration::from_secs(45)).unwrap(),
                ScheduleStatus::Removed,
                ReconcileOutcome::StoredPaused,
            ),
            (
                Schedule::every(Duration::from_secs(60)).unwrap(),
                ScheduleStatus::Expired,
                ReconcileOutcome::Expired,
            ),
            (
                Schedule::every(Duration::from_secs(75)).unwrap(),
                ScheduleStatus::Scheduled,
                ReconcileOutcome::DuplicateStale,
            ),
        ];

        for (schedule, status, outcome) in cases {
            let original = record(schedule, status, outcome);
            let decoded = decode_checkpoint_record(&encode_checkpoint_record(&original).unwrap()).unwrap();
            assert_eq!(decoded.status, status);
            assert_eq!(decoded.last_outcome, outcome);
            match (&decoded.schedule, &original.schedule) {
                (
                    Schedule::RRule {
                        dtstart: a, rrule: ra, ..
                    },
                    Schedule::RRule {
                        dtstart: b, rrule: rb, ..
                    },
                ) => {
                    assert_eq!(a.to_datetime(), b.to_datetime());
                    assert_eq!(ra.as_str(), rb.as_str());
                }
                _ => assert_eq!(decoded.schedule, original.schedule),
            }
        }
    }

    #[test]
    fn corrupt_bytes_are_rejected() {
        assert!(matches!(
            decode_checkpoint_record(b"not proto").unwrap_err(),
            CheckpointCodecError::Wire { .. }
        ));
    }

    #[test]
    fn default_enums_decode_to_stable_domain_values() {
        let stored = checkpoints_v1::ScheduleCheckpoint {
            schedule_id: Some("orders/created".to_string()),
            status: None,
            last_applied_stream_position: Some(1),
            last_applied_event_id: None,
            last_outcome: None,
            schedule: buffa::MessageField::none(),
            delivery: buffa::MessageField::none(),
            message: buffa::MessageField::none(),
        };
        let error = decode_checkpoint_record(&stored.encode_to_vec()).unwrap_err();
        assert!(matches!(error, CheckpointCodecError::Domain { .. }));
    }

    #[test]
    fn unrecognized_enum_values_decode_to_unknown() {
        let original = record(
            Schedule::every(Duration::from_secs(30)).unwrap(),
            ScheduleStatus::Scheduled,
            ReconcileOutcome::Published,
        );
        let mut stored =
            checkpoints_v1::ScheduleCheckpoint::decode_from_slice(&encode_checkpoint_record(&original).unwrap())
                .unwrap();
        stored.status = Some(buffa::EnumValue::from(999));
        stored.last_outcome = Some(buffa::EnumValue::from(999));

        let decoded = decode_checkpoint_record(&stored.encode_to_vec()).unwrap();

        assert_eq!(decoded.status, ScheduleStatus::Unknown);
        assert_eq!(decoded.last_outcome, ReconcileOutcome::Unknown);
    }

    #[test]
    fn missing_status_and_outcome_are_rejected() {
        let schedule = Schedule::every(Duration::from_secs(30)).unwrap();
        let original = record(schedule, ScheduleStatus::Scheduled, ReconcileOutcome::Published);
        let mut stored =
            checkpoints_v1::ScheduleCheckpoint::decode_from_slice(&encode_checkpoint_record(&original).unwrap())
                .unwrap();
        stored.status = None;
        assert!(matches!(
            decode_checkpoint_record(&stored.encode_to_vec()).unwrap_err(),
            CheckpointCodecError::Domain { .. }
        ));

        stored = checkpoints_v1::ScheduleCheckpoint::decode_from_slice(&encode_checkpoint_record(&original).unwrap())
            .unwrap();
        stored.last_outcome = None;
        assert!(matches!(
            decode_checkpoint_record(&stored.encode_to_vec()).unwrap_err(),
            CheckpointCodecError::Domain { .. }
        ));
    }

    #[test]
    fn zero_stream_position_is_rejected() {
        let original = record(
            Schedule::every(Duration::from_secs(30)).unwrap(),
            ScheduleStatus::Scheduled,
            ReconcileOutcome::Published,
        );
        let mut bytes = encode_checkpoint_record(&original).unwrap();
        bytes = rewrite_checkpoint_watermark(&bytes, 0);
        assert!(matches!(
            decode_checkpoint_record(&bytes).unwrap_err(),
            CheckpointCodecError::StreamPosition
        ));
    }

    #[test]
    fn codec_errors_display_and_expose_sources() {
        let wire = decode_checkpoint_record(b"not proto").unwrap_err();
        assert!(
            wire.to_string()
                .starts_with("checkpoint record wire format is invalid:")
        );
        assert!(std::error::Error::source(&wire).is_some());

        let domain = decode_checkpoint_record(
            &checkpoints_v1::ScheduleCheckpoint {
                schedule_id: Some("orders/created".to_string()),
                status: Some(checkpoints_v1::ScheduleCheckpointStatus::Scheduled.into()),
                last_applied_stream_position: Some(1),
                last_applied_event_id: None,
                last_outcome: Some(checkpoints_v1::ReconcileOutcome::Published.into()),
                schedule: buffa::MessageField::none(),
                delivery: buffa::MessageField::none(),
                message: buffa::MessageField::none(),
            }
            .encode_to_vec(),
        )
        .unwrap_err();
        assert!(
            domain
                .to_string()
                .starts_with("checkpoint record snapshot could not be rebuilt:")
        );
        assert!(std::error::Error::source(&domain).is_some());

        let stream_position = CheckpointCodecError::StreamPosition;
        assert_eq!(
            stream_position.to_string(),
            "checkpoint stream position must be greater than zero"
        );
        assert!(std::error::Error::source(&stream_position).is_none());

        let snapshot_conversion = CheckpointCodecError::SnapshotConversion;
        assert_eq!(
            snapshot_conversion.to_string(),
            "checkpoint snapshot could not be encoded to proto"
        );
        assert!(std::error::Error::source(&snapshot_conversion).is_none());
    }

    #[test]
    fn decode_checkpoint_envelope_reads_metadata_without_definition() {
        let original = record(
            Schedule::every(Duration::from_secs(30)).unwrap(),
            ScheduleStatus::Scheduled,
            ReconcileOutcome::Published,
        );
        let bytes = corrupt_checkpoint_schedule(&encode_checkpoint_record(&original).unwrap());
        assert!(decode_checkpoint_record(&bytes).is_err());
        assert_eq!(
            decode_checkpoint_envelope(&bytes),
            CorruptCheckpointEnvelope {
                watermark: Some(StreamPosition::try_new(7).unwrap()),
                last_applied_event_id: Some("event-7".to_string()),
            }
        );
    }

    #[test]
    fn decode_checkpoint_envelope_recovers_fields_parsed_before_a_truncation_point() {
        let original = record(
            Schedule::every(Duration::from_secs(30)).unwrap(),
            ScheduleStatus::Scheduled,
            ReconcileOutcome::Published,
        );
        let bytes = encode_checkpoint_record(&original).unwrap();
        // Truncate inside the trailing nested snapshot, after the envelope fields.
        let truncated = &bytes[..bytes.len() - 5];

        assert!(decode_checkpoint_record(truncated).is_err());
        assert_eq!(
            decode_checkpoint_envelope(truncated),
            CorruptCheckpointEnvelope {
                watermark: Some(StreamPosition::try_new(7).unwrap()),
                last_applied_event_id: Some("event-7".to_string()),
            }
        );
    }

    #[test]
    fn decode_checkpoint_envelope_keeps_watermark_when_event_id_is_invalid_utf8() {
        let original = record(
            Schedule::every(Duration::from_secs(30)).unwrap(),
            ScheduleStatus::Scheduled,
            ReconcileOutcome::Published,
        );
        let bytes = corrupt_checkpoint_event_id(&encode_checkpoint_record(&original).unwrap());
        assert_eq!(
            decode_checkpoint_envelope(&bytes),
            CorruptCheckpointEnvelope {
                watermark: Some(StreamPosition::try_new(7).unwrap()),
                last_applied_event_id: None,
            }
        );
    }

    #[test]
    fn decode_checkpoint_envelope_keeps_event_id_when_watermark_is_missing_or_invalid() {
        let original = record(
            Schedule::every(Duration::from_secs(30)).unwrap(),
            ScheduleStatus::Scheduled,
            ReconcileOutcome::Published,
        );
        let bytes = rewrite_checkpoint_watermark(&encode_checkpoint_record(&original).unwrap(), 0);

        assert_eq!(
            decode_checkpoint_envelope(&bytes),
            CorruptCheckpointEnvelope {
                watermark: None,
                last_applied_event_id: Some("event-7".to_string()),
            }
        );
    }
}
