CREATE TABLE IF NOT EXISTS agent_traffic_events (
    event_id    TEXT PRIMARY KEY,
    ts          TIMESTAMPTZ NOT NULL,
    tenant      TEXT NOT NULL,
    caller_sub  TEXT,
    caller_wkl  TEXT,
    target_aud  TEXT,
    purpose     TEXT,
    scope       TEXT,
    outcome     TEXT NOT NULL,
    reason      TEXT,
    act_chain   JSONB,
    request_id  TEXT,
    session_id  TEXT
);

CREATE INDEX IF NOT EXISTS idx_agent_traffic_tenant_ts
    ON agent_traffic_events (tenant, ts DESC);

CREATE INDEX IF NOT EXISTS idx_agent_traffic_tenant_caller_sub_ts
    ON agent_traffic_events (tenant, caller_sub, ts DESC);

CREATE INDEX IF NOT EXISTS idx_agent_traffic_session
    ON agent_traffic_events (tenant, session_id, ts DESC);
