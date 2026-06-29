-- Schedules read-model projection (Postgres backend).
--
-- A denormalized read model (one row per schedule), but with every scalar field of
-- the schedule promoted to a typed, indexable column rather than buried in a blob.
-- Only the genuinely repeated parts stay non-scalar: `rrule_rdate`/`rrule_exdate`
-- as timestamp arrays and `message_headers` as JSONB. The schedule and delivery
-- oneofs are flattened to a `*_kind` discriminator plus the variant's columns.
CREATE TABLE IF NOT EXISTS schedules_projection (
    schedule_id             TEXT PRIMARY KEY,
    status                  TEXT NOT NULL,            -- scheduled | paused
    completed               BOOLEAN NOT NULL DEFAULT FALSE,
    next_occurrence_at      TIMESTAMPTZ,
    last_occurrence_at      TIMESTAMPTZ,

    -- schedule spec (oneof: at | every | cron | rrule)
    schedule_kind           TEXT NOT NULL CHECK (schedule_kind IN ('at', 'every', 'cron', 'rrule')),
    at_at                   TIMESTAMPTZ,
    every_seconds           BIGINT,
    cron_expr               TEXT,
    rrule                   TEXT,
    rrule_dtstart           TIMESTAMPTZ,
    timezone                TEXT,                     -- shared by cron and rrule
    rrule_rdate             TIMESTAMPTZ[] NOT NULL DEFAULT '{}',
    rrule_exdate            TIMESTAMPTZ[] NOT NULL DEFAULT '{}',

    -- delivery (oneof: nats_message)
    delivery_kind           TEXT NOT NULL CHECK (delivery_kind IN ('nats_message')),
    delivery_subject        TEXT,
    delivery_ttl_seconds    BIGINT,
    delivery_source_subject TEXT,                     -- LatestFromSubject sampling source

    -- message (body is raw bytes so non-UTF-8 payloads round-trip losslessly)
    message_content_type    TEXT,
    message_body            BYTEA,
    message_headers         JSONB NOT NULL DEFAULT '[]',

    inserted_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- The execution worker arms the nearest pending occurrence, so ordering and
-- filtering by `next_occurrence_at` is the listing's hot path.
CREATE INDEX IF NOT EXISTS schedules_projection_next_occurrence_idx
    ON schedules_projection (next_occurrence_at);
-- Common groupings now that the fields are real columns.
CREATE INDEX IF NOT EXISTS schedules_projection_kind_idx
    ON schedules_projection (schedule_kind);
CREATE INDEX IF NOT EXISTS schedules_projection_delivery_subject_idx
    ON schedules_projection (delivery_subject);

-- The catch-up checkpoint lives in `jetstream_projection_checkpoint` (migration
-- 0001), keyed by this projection's id.
