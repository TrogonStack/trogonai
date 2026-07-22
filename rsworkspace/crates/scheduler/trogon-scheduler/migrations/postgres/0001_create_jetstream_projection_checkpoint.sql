-- Catch-up checkpoint for JetStream-fed projections: the last fully folded
-- JetStream sequence, keyed by a projection id string (the same way the NATS
-- backend keys its checkpoint). Generic across projections: one row per id.
CREATE TABLE IF NOT EXISTS jetstream_projection_checkpoint (
    id                  TEXT PRIMARY KEY,
    last_event_sequence BIGINT NOT NULL
);
