use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

use crate::envelope::AuditEnvelope;
use crate::error::{IndexerError, ProjectorError};
use crate::event::{ActChainHop, TrafficEvent, TrafficQueryFilter, TrafficSource};
use crate::indexer::TrafficIndex;
use crate::projector::{normalize, AuditProjector};

pub struct PostgresIndexer {
    pool: PgPool,
}

impl PostgresIndexer {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn connect(database_url: &str) -> Result<Self, IndexerError> {
        let pool = PgPool::connect(database_url).await?;
        Ok(Self::new(pool))
    }

    pub async fn migrate(&self) -> Result<(), IndexerError> {
        for statement in include_str!("../../migrations/001_agent_traffic_events.sql")
            .split(';')
            .map(str::trim)
            .filter(|statement| !statement.is_empty())
        {
            sqlx::query(statement).execute(&self.pool).await?;
        }
        Ok(())
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl AuditProjector for PostgresIndexer {
    async fn project(&self, envelope: AuditEnvelope) -> Result<(), ProjectorError> {
        let event = normalize::normalize_envelope(&envelope)?;
        self.put_event(event).await?;
        Ok(())
    }
}

#[async_trait]
impl TrafficIndex for PostgresIndexer {
    async fn put_event(&self, event: TrafficEvent) -> Result<(), IndexerError> {
        let act_chain = event
            .act_chain
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|error| IndexerError::Query(error.to_string()))?;

        sqlx::query(
            r#"
            INSERT INTO agent_traffic_events (
                event_id, ts, tenant, caller_sub, caller_wkl, target_aud,
                purpose, scope, outcome, reason, act_chain, request_id, session_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            ON CONFLICT (event_id) DO NOTHING
            "#,
        )
        .bind(&event.event_id)
        .bind(event.ts)
        .bind(&event.tenant)
        .bind(&event.caller_sub)
        .bind(&event.caller_wkl)
        .bind(&event.target_aud)
        .bind(&event.purpose)
        .bind(&event.scope)
        .bind(&event.outcome)
        .bind(&event.reason)
        .bind(act_chain)
        .bind(&event.request_id)
        .bind(&event.session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn query(&self, filter: TrafficQueryFilter) -> Result<Vec<TrafficEvent>, IndexerError> {
        let tenant = filter
            .tenant
            .ok_or_else(|| IndexerError::Query("tenant is required".into()))?;
        let limit = filter.limit.unwrap_or(100).min(1_000) as i64;
        let since = filter.since.unwrap_or_else(|| Utc::now() - chrono::Duration::hours(1));
        let until = filter.until.unwrap_or_else(Utc::now);

        let rows = if let Some(agent_id) = filter.agent_id {
            sqlx::query(
                r#"
                SELECT event_id, ts, tenant, caller_sub, caller_wkl, target_aud, purpose, scope,
                       outcome, reason, act_chain, request_id, session_id
                FROM agent_traffic_events
                WHERE tenant = $1
                  AND ts >= $2
                  AND ts < $3
                  AND (caller_sub = $4 OR caller_sub LIKE $5)
                ORDER BY ts DESC
                LIMIT $6
                "#,
            )
            .bind(&tenant)
            .bind(since)
            .bind(until)
            .bind(&agent_id)
            .bind(format!("agent:{agent_id}"))
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT event_id, ts, tenant, caller_sub, caller_wkl, target_aud, purpose, scope,
                       outcome, reason, act_chain, request_id, session_id
                FROM agent_traffic_events
                WHERE tenant = $1 AND ts >= $2 AND ts < $3
                ORDER BY ts DESC
                LIMIT $4
                "#,
            )
            .bind(&tenant)
            .bind(since)
            .bind(until)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        };

        rows.into_iter().map(row_to_event).collect()
    }

    async fn tail(
        &self,
        tenant: &str,
        since: Option<DateTime<Utc>>,
        limit: u32,
    ) -> Result<Vec<TrafficEvent>, IndexerError> {
        let since = since.unwrap_or_else(|| Utc::now() - chrono::Duration::minutes(5));
        let limit = limit.min(1_000) as i64;
        let rows = sqlx::query(
            r#"
            SELECT event_id, ts, tenant, caller_sub, caller_wkl, target_aud, purpose, scope,
                   outcome, reason, act_chain, request_id, session_id
            FROM agent_traffic_events
            WHERE tenant = $1 AND ts >= $2
            ORDER BY ts ASC
            LIMIT $3
            "#,
        )
        .bind(tenant)
        .bind(since)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_event).collect()
    }
}

fn row_to_event(row: sqlx::postgres::PgRow) -> Result<TrafficEvent, IndexerError> {
    let act_chain: Option<serde_json::Value> = row.try_get("act_chain")?;
    let act_chain = act_chain
        .map(serde_json::from_value::<Vec<ActChainHop>>)
        .transpose()
        .map_err(|error| IndexerError::Query(error.to_string()))?;

    Ok(TrafficEvent {
        event_id: row.try_get("event_id")?,
        ts: row.try_get("ts")?,
        tenant: row.try_get("tenant")?,
        caller_sub: row.try_get("caller_sub")?,
        caller_wkl: row.try_get("caller_wkl")?,
        target_aud: row.try_get("target_aud")?,
        purpose: row.try_get("purpose")?,
        scope: row.try_get("scope")?,
        outcome: row.try_get("outcome")?,
        reason: row.try_get("reason")?,
        act_chain,
        request_id: row.try_get("request_id")?,
        session_id: row.try_get("session_id")?,
        source: TrafficSource::Sts,
    })
}
