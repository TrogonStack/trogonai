//! The Postgres backend for the schedules read model (the `postgres` feature).
//!
//! An alternative to the default NATS KV backend. Rather than storing the
//! `projections.v1.ScheduleProjection` proto as an opaque blob, every scalar field
//! is normalized into a typed, indexable column of `schedules_projection` (keyed by
//! the raw schedule id — Postgres has no KV-key character restrictions). Only the
//! repeated parts stay non-scalar: `rrule_rdate`/`rrule_exdate` as timestamp arrays
//! and `message_headers` as JSONB. The schedule and delivery oneofs are flattened
//! to a `*_kind` discriminator plus the variant's columns.
//!
//! The trait still speaks the proto (the fold and the query side both operate on
//! it), so this backend decomposes the proto into columns on write and recomposes
//! it from the row on read. The catch-up checkpoint lives in the shared
//! `jetstream_projection_checkpoint` table, keyed by [`SCHEDULES_CHECKPOINT_ID`].

#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use std::collections::HashSet;

use async_trait::async_trait;
use buffa::MessageField;
use buffa_types::google::protobuf::{Duration, Timestamp};
use chrono::{DateTime, TimeZone, Utc};
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{PgPool, Row};

use crate::{error::SchedulerError, projections_v1};

use super::SchedulesProjectionStore;

use projections_v1::__buffa::oneof::delivery::Kind as DeliveryKind;
use projections_v1::__buffa::oneof::delivery::nats_message::source::Kind as SourceKind;
use projections_v1::__buffa::oneof::schedule::Kind as ScheduleKind;
use projections_v1::__buffa::oneof::schedule_status::Kind as ScheduleStatusKind;

/// This projection's id in the shared `jetstream_projection_checkpoint` table.
const SCHEDULES_CHECKPOINT_ID: &str = "schedules_read_model";

/// A schedules read model stored in a Postgres table.
#[derive(Clone)]
pub struct PostgresSchedulesProjection {
    pool: PgPool,
}

impl PostgresSchedulesProjection {
    /// Connects a pool to `database_url`, applies the embedded migrations, and
    /// returns a ready backend.
    pub async fn connect(database_url: &str) -> Result<Self, SchedulerError> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .map_err(|source| SchedulerError::kv_source("failed to connect schedules Postgres pool", source))?;
        Self::run_migrations(&pool).await?;
        Ok(Self { pool })
    }

    /// Builds a backend over an existing pool without running migrations. The
    /// caller is responsible for having applied [`Self::run_migrations`].
    pub const fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Applies the embedded schema migrations to `pool`.
    pub async fn run_migrations(pool: &PgPool) -> Result<(), SchedulerError> {
        sqlx::migrate!("./migrations/postgres")
            .run(pool)
            .await
            .map_err(|source| SchedulerError::kv_source("failed to run schedules Postgres migrations", source))
    }

    pub const fn pool(&self) -> &PgPool {
        &self.pool
    }
}

const SELECT_COLUMNS: &str = "SELECT schedule_id, status, completed, next_occurrence_at, last_occurrence_at, \
     schedule_kind, at_at, every_seconds, cron_expr, rrule, rrule_dtstart, timezone, rrule_rdate, rrule_exdate, \
     delivery_kind, delivery_subject, delivery_ttl_seconds, delivery_source_subject, \
     message_content_type, message_body, message_headers \
     FROM schedules_projection";

// ── error helpers ───────────────────────────────────────────────────────────

fn malformed(context: &'static str) -> SchedulerError {
    SchedulerError::kv_source("projected schedule view is malformed", std::io::Error::other(context))
}

fn malformed_owned(context: String) -> SchedulerError {
    SchedulerError::kv_source("projected schedule view is malformed", std::io::Error::other(context))
}

fn col<'r, T>(row: &'r PgRow, name: &'static str) -> Result<T, SchedulerError>
where
    T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get::<T, _>(name)
        .map_err(|source| SchedulerError::kv_source("failed to read projected schedule column", source))
}

// ── proto <-> scalar conversions ──────────────────────────────────────────────

fn timestamp_to_datetime(ts: &Timestamp) -> DateTime<Utc> {
    Utc.timestamp_opt(ts.seconds, ts.nanos as u32).single().unwrap_or_default()
}

fn datetime_to_timestamp(dt: &DateTime<Utc>) -> Timestamp {
    Timestamp {
        seconds: dt.timestamp(),
        nanos: dt.timestamp_subsec_nanos() as i32,
        ..Default::default()
    }
}

fn optional_timestamp(dt: Option<DateTime<Utc>>) -> MessageField<Timestamp> {
    dt.map_or_else(MessageField::none, |dt| MessageField::some(datetime_to_timestamp(&dt)))
}

/// Schedule intervals/TTLs are stored as whole seconds (`BIGINT`); sub-second
/// precision is dropped, which is fine for a second-granularity read model.
fn optional_duration(seconds: Option<i64>) -> MessageField<Duration> {
    seconds.map_or_else(MessageField::none, |seconds| {
        MessageField::some(Duration {
            seconds,
            ..Default::default()
        })
    })
}

fn timezone_text(tz: &MessageField<trogonai_proto::google::r#type::TimeZone>) -> Option<String> {
    tz.as_option().map(|tz| tz.id.clone()).filter(|id| !id.is_empty())
}

fn optional_timezone(id: Option<String>) -> MessageField<trogonai_proto::google::r#type::TimeZone> {
    match id.filter(|id| !id.is_empty()) {
        Some(id) => MessageField::some(trogonai_proto::google::r#type::TimeZone {
            id,
            ..Default::default()
        }),
        None => MessageField::none(),
    }
}

fn status_text(view: &projections_v1::ScheduleProjection) -> &'static str {
    if matches!(
        view.status.as_option().and_then(|status| status.kind.as_ref()),
        Some(ScheduleStatusKind::Paused(_))
    ) {
        "paused"
    } else {
        "scheduled"
    }
}

fn status_proto(status: &str) -> projections_v1::ScheduleStatus {
    let kind = if status == "paused" {
        projections_v1::schedule_status::Paused {}.into()
    } else {
        projections_v1::schedule_status::Scheduled {}.into()
    };
    projections_v1::ScheduleStatus { kind: Some(kind) }
}

fn source_subject(source: &projections_v1::delivery::nats_message::Source) -> Option<String> {
    source
        .kind
        .as_ref()
        .map(|SourceKind::LatestFromSubject(inner)| inner.subject.clone())
}

fn headers_to_json(view: &projections_v1::ScheduleProjection) -> serde_json::Value {
    let headers = view.message.as_option().map(|message| message.headers.as_slice()).unwrap_or(&[]);
    serde_json::Value::Array(
        headers
            .iter()
            .map(|header| serde_json::json!({ "name": header.name, "value": header.value }))
            .collect(),
    )
}

fn headers_from_json(value: &serde_json::Value) -> Vec<projections_v1::Header> {
    value
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    Some(projections_v1::Header {
                        name: entry.get("name")?.as_str()?.to_string(),
                        value: entry.get("value")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

// ── row -> proto (read side) ──────────────────────────────────────────────────

fn view_from_row(row: &PgRow) -> Result<projections_v1::ScheduleProjection, SchedulerError> {
    let timezone: Option<String> = col(row, "timezone")?;
    let schedule_kind: String = col(row, "schedule_kind")?;
    let schedule: ScheduleKind = match schedule_kind.as_str() {
        "at" => projections_v1::schedule::At {
            at: optional_timestamp(col(row, "at_at")?),
        }
        .into(),
        "every" => projections_v1::schedule::Every {
            every: optional_duration(col(row, "every_seconds")?),
        }
        .into(),
        "cron" => projections_v1::schedule::Cron {
            expr: col::<Option<String>>(row, "cron_expr")?.unwrap_or_default(),
            timezone: optional_timezone(timezone),
        }
        .into(),
        "rrule" => {
            let rdate: Vec<DateTime<Utc>> = col(row, "rrule_rdate")?;
            let exdate: Vec<DateTime<Utc>> = col(row, "rrule_exdate")?;
            projections_v1::schedule::RRule {
                dtstart: optional_timestamp(col(row, "rrule_dtstart")?),
                rrule: col::<Option<String>>(row, "rrule")?.unwrap_or_default(),
                timezone: optional_timezone(timezone),
                rdate: rdate.iter().map(datetime_to_timestamp).collect(),
                exdate: exdate.iter().map(datetime_to_timestamp).collect(),
            }
            .into()
        }
        other => return Err(malformed_owned(format!("unknown schedule_kind '{other}'"))),
    };

    let delivery_kind: String = col(row, "delivery_kind")?;
    let delivery: DeliveryKind = match delivery_kind.as_str() {
        "nats_message" => projections_v1::delivery::NatsMessage {
            subject: col::<Option<String>>(row, "delivery_subject")?.unwrap_or_default(),
            ttl: optional_duration(col(row, "delivery_ttl_seconds")?),
            source: col::<Option<String>>(row, "delivery_source_subject")?.map_or_else(MessageField::none, |subject| {
                MessageField::some(projections_v1::delivery::nats_message::Source {
                    kind: Some(projections_v1::delivery::nats_message::LatestFromSubject { subject }.into()),
                })
            }),
        }
        .into(),
        other => return Err(malformed_owned(format!("unknown delivery_kind '{other}'"))),
    };

    let content_type: Option<String> = col(row, "message_content_type")?;
    let body: Option<String> = col(row, "message_body")?;
    let headers_json: serde_json::Value = col(row, "message_headers")?;
    let content = match (content_type, body) {
        (None, None) => MessageField::none(),
        (content_type, body) => MessageField::some(trogonai_proto::content::v1alpha1::Content {
            content_type: content_type.unwrap_or_default(),
            data: body.unwrap_or_default().into_bytes(),
        }),
    };

    let status: String = col(row, "status")?;
    Ok(projections_v1::ScheduleProjection {
        schedule_id: col(row, "schedule_id")?,
        status: MessageField::some(status_proto(&status)),
        completed: Some(col(row, "completed")?),
        next_occurrence_at: optional_timestamp(col(row, "next_occurrence_at")?),
        last_occurrence_at: optional_timestamp(col(row, "last_occurrence_at")?),
        schedule: MessageField::some(projections_v1::Schedule { kind: Some(schedule) }),
        delivery: MessageField::some(projections_v1::Delivery { kind: Some(delivery) }),
        message: MessageField::some(projections_v1::Message {
            content,
            headers: headers_from_json(&headers_json),
        }),
    })
}

#[async_trait]
impl SchedulesProjectionStore for PostgresSchedulesProjection {
    async fn get_view(
        &self,
        schedule_id: &str,
    ) -> Result<Option<projections_v1::ScheduleProjection>, SchedulerError> {
        let row = sqlx::query(&format!("{SELECT_COLUMNS} WHERE schedule_id = $1"))
            .bind(schedule_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|source| SchedulerError::kv_source("failed to read projected schedule", source))?;
        match row {
            Some(row) => view_from_row(&row).map(Some),
            None => Ok(None),
        }
    }

    async fn list_views(&self) -> Result<Vec<projections_v1::ScheduleProjection>, SchedulerError> {
        let rows = sqlx::query(&format!("{SELECT_COLUMNS} ORDER BY schedule_id"))
            .fetch_all(&self.pool)
            .await
            .map_err(|source| SchedulerError::kv_source("failed to list projected schedules", source))?;
        let mut views = Vec::with_capacity(rows.len());
        for row in &rows {
            // One corrupt row must not suppress every other schedule in the listing.
            match view_from_row(row) {
                Ok(view) => views.push(view),
                Err(source) => {
                    let key: String = row.try_get("schedule_id").unwrap_or_default();
                    tracing::warn!(%key, %source, "skipping unreadable projected schedule row during list");
                }
            }
        }
        Ok(views)
    }

    async fn upsert_view(&self, view: &projections_v1::ScheduleProjection) -> Result<(), SchedulerError> {
        let schedule = view
            .schedule
            .as_option()
            .and_then(|schedule| schedule.kind.as_ref())
            .ok_or_else(|| malformed("missing schedule"))?;
        #[allow(clippy::type_complexity)]
        let (schedule_kind, at_at, every_seconds, cron_expr, rrule, rrule_dtstart, timezone, rrule_rdate, rrule_exdate): (
            &str,
            Option<DateTime<Utc>>,
            Option<i64>,
            Option<String>,
            Option<String>,
            Option<DateTime<Utc>>,
            Option<String>,
            Vec<DateTime<Utc>>,
            Vec<DateTime<Utc>>,
        ) = match schedule {
            ScheduleKind::At(inner) => ("at", inner.at.as_option().map(timestamp_to_datetime), None, None, None, None, None, vec![], vec![]),
            ScheduleKind::Every(inner) => ("every", None, inner.every.as_option().map(|d| d.seconds), None, None, None, None, vec![], vec![]),
            ScheduleKind::Cron(inner) => (
                "cron",
                None,
                None,
                Some(inner.expr.clone()),
                None,
                None,
                timezone_text(&inner.timezone),
                vec![],
                vec![],
            ),
            ScheduleKind::Rrule(inner) => (
                "rrule",
                None,
                None,
                None,
                Some(inner.rrule.clone()),
                inner.dtstart.as_option().map(timestamp_to_datetime),
                timezone_text(&inner.timezone),
                inner.rdate.iter().map(timestamp_to_datetime).collect(),
                inner.exdate.iter().map(timestamp_to_datetime).collect(),
            ),
        };

        let delivery = view
            .delivery
            .as_option()
            .and_then(|delivery| delivery.kind.as_ref())
            .ok_or_else(|| malformed("missing delivery"))?;
        let (delivery_kind, delivery_subject, delivery_ttl_seconds, delivery_source_subject): (
            &str,
            Option<String>,
            Option<i64>,
            Option<String>,
        ) = match delivery {
            DeliveryKind::NatsMessage(inner) => (
                "nats_message",
                Some(inner.subject.clone()),
                inner.ttl.as_option().map(|ttl| ttl.seconds),
                inner.source.as_option().and_then(source_subject),
            ),
        };

        let (message_content_type, message_body) = match view.message.as_option().and_then(|m| m.content.as_option()) {
            Some(content) => (
                Some(content.content_type.clone()),
                Some(String::from_utf8_lossy(&content.data).into_owned()),
            ),
            None => (None, None),
        };
        let message_headers = headers_to_json(view);

        sqlx::query(
            "INSERT INTO schedules_projection ( \
                 schedule_id, status, completed, next_occurrence_at, last_occurrence_at, \
                 schedule_kind, at_at, every_seconds, cron_expr, rrule, rrule_dtstart, timezone, rrule_rdate, rrule_exdate, \
                 delivery_kind, delivery_subject, delivery_ttl_seconds, delivery_source_subject, \
                 message_content_type, message_body, message_headers \
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21) \
             ON CONFLICT (schedule_id) DO UPDATE SET \
                 status = EXCLUDED.status, \
                 completed = EXCLUDED.completed, \
                 next_occurrence_at = EXCLUDED.next_occurrence_at, \
                 last_occurrence_at = EXCLUDED.last_occurrence_at, \
                 schedule_kind = EXCLUDED.schedule_kind, \
                 at_at = EXCLUDED.at_at, \
                 every_seconds = EXCLUDED.every_seconds, \
                 cron_expr = EXCLUDED.cron_expr, \
                 rrule = EXCLUDED.rrule, \
                 rrule_dtstart = EXCLUDED.rrule_dtstart, \
                 timezone = EXCLUDED.timezone, \
                 rrule_rdate = EXCLUDED.rrule_rdate, \
                 rrule_exdate = EXCLUDED.rrule_exdate, \
                 delivery_kind = EXCLUDED.delivery_kind, \
                 delivery_subject = EXCLUDED.delivery_subject, \
                 delivery_ttl_seconds = EXCLUDED.delivery_ttl_seconds, \
                 delivery_source_subject = EXCLUDED.delivery_source_subject, \
                 message_content_type = EXCLUDED.message_content_type, \
                 message_body = EXCLUDED.message_body, \
                 message_headers = EXCLUDED.message_headers, \
                 updated_at = now()",
        )
        .bind(&view.schedule_id)
        .bind(status_text(view))
        .bind(view.completed.unwrap_or(false))
        .bind(view.next_occurrence_at.as_option().map(timestamp_to_datetime))
        .bind(view.last_occurrence_at.as_option().map(timestamp_to_datetime))
        .bind(schedule_kind)
        .bind(at_at)
        .bind(every_seconds)
        .bind(cron_expr)
        .bind(rrule)
        .bind(rrule_dtstart)
        .bind(timezone)
        .bind(rrule_rdate)
        .bind(rrule_exdate)
        .bind(delivery_kind)
        .bind(delivery_subject)
        .bind(delivery_ttl_seconds)
        .bind(delivery_source_subject)
        .bind(message_content_type)
        .bind(message_body)
        .bind(message_headers)
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(|source| SchedulerError::kv_source("failed to store projected job state", source))
    }

    async fn delete_view(&self, schedule_id: &str) -> Result<(), SchedulerError> {
        sqlx::query("DELETE FROM schedules_projection WHERE schedule_id = $1")
            .bind(schedule_id)
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(|source| SchedulerError::kv_source("failed to delete projected job state", source))
    }

    async fn reconcile(&self, live_ids: &HashSet<String>) -> Result<(), SchedulerError> {
        // `schedule_id <> ALL($1)` deletes every row absent from the folded state;
        // an empty array makes the predicate true for all rows, clearing the table.
        let ids: Vec<String> = live_ids.iter().cloned().collect();
        sqlx::query("DELETE FROM schedules_projection WHERE schedule_id <> ALL($1)")
            .bind(&ids)
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(|source| SchedulerError::kv_source("failed to reconcile schedules read model", source))
    }

    async fn read_checkpoint(&self) -> Result<u64, SchedulerError> {
        let row = sqlx::query("SELECT last_event_sequence FROM jetstream_projection_checkpoint WHERE id = $1")
            .bind(SCHEDULES_CHECKPOINT_ID)
            .fetch_optional(&self.pool)
            .await
            .map_err(|source| SchedulerError::kv_source("failed to read schedules read-model checkpoint", source))?;
        let Some(row) = row else {
            return Ok(0);
        };
        let sequence: i64 = row
            .try_get("last_event_sequence")
            .map_err(|source| SchedulerError::kv_source("failed to read schedules checkpoint column", source))?;
        Ok(sequence.max(0) as u64)
    }

    async fn write_checkpoint(&self, sequence: u64) -> Result<(), SchedulerError> {
        sqlx::query(
            "INSERT INTO jetstream_projection_checkpoint (id, last_event_sequence) \
             VALUES ($1, $2) \
             ON CONFLICT (id) DO UPDATE SET last_event_sequence = EXCLUDED.last_event_sequence",
        )
        .bind(SCHEDULES_CHECKPOINT_ID)
        .bind(sequence as i64)
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(|source| SchedulerError::kv_source("failed to write schedules read-model checkpoint", source))
    }
}
