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
        tracing::info!(
            event = "sql_query_admission",
            user = user.as_str(),
            client = client.as_str(),
            connection = connection.map(ObjectId::as_str)
        );
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
        tracing::info!(event="sql_query_completion",user=self.user.as_str(),client=self.client.as_str(),connection=self.connection.map(ObjectId::as_str),outcome=?self.outcome,elapsed_ms=self.started.elapsed().as_millis() as u64);
    }
}
pub(crate) fn references(resources: &BTreeSet<Resource>, user: &ObjectId, client: &ObjectId, connection: &ObjectId) {
    for resource in resources {
        tracing::info!(
            event = "sql_query_reference",
            user = user.as_str(),
            client = client.as_str(),
            connection = connection.as_str(),
            resource_kind = resource.kind,
            resource_id = resource.id.as_str()
        );
    }
}
