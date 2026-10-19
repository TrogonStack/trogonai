use crate::{Error, ObjectId, Resource};
use std::{collections::BTreeSet, time::Instant};

#[derive(Debug)]
enum QueryOutcome {
    Cancelled,
    Permitted,
    Denied,
    Timeout,
    Limited,
    Rejected,
    Unavailable,
}

pub(crate) struct QueryAudit<'a> {
    user: &'a ObjectId,
    client: &'a ObjectId,
    connection: Option<&'a ObjectId>,
    started: Instant,
    outcome: QueryOutcome,
}
impl<'a> QueryAudit<'a> {
    pub(crate) fn start(user: &'a ObjectId, client: &'a ObjectId, connection: Option<&'a ObjectId>) -> Self {
        {
            let user = user.as_str();
            let client = client.as_str();
            let connection = connection.map(ObjectId::as_str);
            tracing::info!(event = "sql_query_admission", user, client, connection);
        }
        Self {
            user,
            client,
            connection,
            started: Instant::now(),
            outcome: QueryOutcome::Cancelled,
        }
    }
    pub(crate) fn complete<T>(&mut self, result: &Result<T, Error>) {
        self.outcome = match result {
            Ok(_) => QueryOutcome::Permitted,
            Err(Error::Denied) => QueryOutcome::Denied,
            Err(Error::Timeout) => QueryOutcome::Timeout,
            Err(Error::Limit) => QueryOutcome::Limited,
            Err(
                Error::UnsupportedSql
                | Error::UnsupportedObject
                | Error::Parse(_)
                | Error::UnknownConnection
                | Error::Identifier,
            ) => QueryOutcome::Rejected,
            Err(_) => QueryOutcome::Unavailable,
        };
    }
}
impl Drop for QueryAudit<'_> {
    fn drop(&mut self) {
        let user = self.user.as_str();
        let client = self.client.as_str();
        let connection = self.connection.map(ObjectId::as_str);
        let elapsed_ms = self.started.elapsed().as_millis() as u64;
        tracing::info!(event = "sql_query_completion", user, client, connection, outcome = ?self.outcome, elapsed_ms);
    }
}
pub(crate) fn references(resources: &BTreeSet<Resource>, user: &ObjectId, client: &ObjectId, connection: &ObjectId) {
    let user = user.as_str();
    let client = client.as_str();
    let connection = connection.as_str();
    for resource in resources {
        let resource_kind = resource.kind;
        let resource_id = resource.id.as_str();
        tracing::info!(
            event = "sql_query_reference",
            user,
            client,
            connection,
            resource_kind,
            resource_id
        );
    }
}
