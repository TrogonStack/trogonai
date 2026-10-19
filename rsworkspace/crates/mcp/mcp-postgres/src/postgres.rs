use crate::{
    Error, ObjectId, PermissionCheck, Query, Resource, RuntimeConfig,
    parser::{Column, Expr},
};
use futures_util::TryStreamExt;
use serde::Serialize;
use sqlx::{
    AssertSqlSafe, Column as _, Connection, Executor, PgConnection, PgPool, Row, SqlSafeStr, Statement, TypeInfo,
    postgres::{PgColumn, PgPoolOptions, PgRow, types::Oid},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Semaphore;

use crate::constants::MAX_BYTES;
#[derive(Serialize, Debug)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
}
impl QueryResult {
    pub(crate) fn into_json(self) -> serde_json::Value {
        serde_json::Value::Object(serde_json::Map::from_iter([
            (
                "columns".into(),
                serde_json::Value::Array(self.columns.into_iter().map(serde_json::Value::String).collect()),
            ),
            (
                "rows".into(),
                serde_json::Value::Array(self.rows.into_iter().map(serde_json::Value::Array).collect()),
            ),
        ]))
    }

    fn check_size(&self) -> Result<(), Error> {
        if serde_json::to_vec(self).map_err(|_| Error::Limit)?.len() > MAX_BYTES {
            return Err(Error::Limit);
        }
        Ok(())
    }
}
pub struct QueryService {
    user: ObjectId,
    client: ObjectId,
    connections: BTreeMap<String, (ObjectId, PgPool)>,
    permissions: Arc<dyn PermissionCheck>,
    permits: Semaphore,
}
#[derive(Debug)]
struct CatalogColumn {
    table: usize,
    name: String,
    attnum: i16,
    type_oid: u32,
}
impl QueryService {
    pub async fn connect(config: RuntimeConfig, permissions: Arc<dyn PermissionCheck>) -> Result<Self, Error> {
        let mut connections = BTreeMap::new();
        for (name, connection) in config.connections {
            let pool = PgPoolOptions::new()
                .max_connections(4)
                .acquire_timeout(Duration::from_secs(2))
                .connect_with(connection.options.into_inner())
                .await?;
            connections.insert(name, (connection.id.into_object_id(), pool));
        }
        Ok(Self {
            user: config.user,
            client: config.client,
            connections,
            permissions,
            permits: Semaphore::new(4),
        })
    }
    pub async fn query(&self, connection: &str, sql: &str) -> Result<QueryResult, Error> {
        self.query_with_parameters(connection, sql, &[]).await
    }
    pub async fn query_with_parameters(
        &self,
        connection: &str,
        sql: &str,
        parameters: &[crate::Parameter],
    ) -> Result<QueryResult, Error> {
        let mut audit = crate::telemetry::QueryAudit::start(
            &self.user,
            &self.client,
            self.connections.get(connection).map(|(id, _)| id),
        );
        let result = async {
            if parameters.len() > 32 {
                return Err(Error::Limit);
            }
            let _permit = self.permits.try_acquire().map_err(|_| Error::Limit)?;
            tokio::time::timeout(
                Duration::from_secs(10),
                self.execute(connection, Query::parse(sql)?, parameters),
            )
            .await
            .map_err(|_| Error::Timeout)?
        }
        .await;
        audit.complete(&result);
        result
    }
    async fn authorize(&self, resource: &Resource) -> Result<(), Error> {
        self.permissions.check(resource, "sql_user", &self.user).await?;
        self.permissions.check(resource, "sql_client", &self.client).await
    }
    async fn execute(
        &self,
        connection: &str,
        query: Query,
        parameters: &[crate::Parameter],
    ) -> Result<QueryResult, Error> {
        let (connection_id, pool) = self.connections.get(connection).ok_or(Error::UnknownConnection)?;
        self.authorize(&Resource {
            kind: "sql_connection",
            id: connection_id.clone(),
        })
        .await?;
        let mut connection = pool.acquire().await?;
        connection.close_on_drop();
        let mut tx = connection.begin().await?;
        tx.execute("SET TRANSACTION ISOLATION LEVEL READ COMMITTED READ ONLY")
            .await?;
        for setting in [
            "SET LOCAL search_path = pg_catalog",
            "SET LOCAL statement_timeout = '5s'",
            "SET LOCAL lock_timeout = '1s'",
            "SET LOCAL idle_in_transaction_session_timeout = '5s'",
            "SET LOCAL standard_conforming_strings = on",
            "SET LOCAL row_security = on",
            "SET LOCAL enable_indexscan = off",
            "SET LOCAL enable_indexonlyscan = off",
            "SET LOCAL enable_bitmapscan = off",
            "SET LOCAL constraint_exclusion = off",
            "SET LOCAL jit = off",
            "SET LOCAL client_connection_check_interval = '100ms'",
        ] {
            tx.execute(setting).await?;
        }
        let unsafe_role: bool = sqlx::query_scalar("SELECT rolsuper OR rolbypassrls OR rolcreaterole OR rolcreatedb OR rolreplication FROM pg_catalog.pg_roles WHERE rolname = current_user").fetch_one(&mut *tx).await?;
        if unsafe_role {
            return Err(Error::UnsupportedObject);
        }
        let database: i64 =
            sqlx::query_scalar("SELECT oid::bigint FROM pg_catalog.pg_database WHERE datname=current_database()")
                .fetch_one(&mut *tx)
                .await?;
        let mut catalog = Vec::new();
        let mut relations = Vec::new();
        let mut resources = BTreeSet::new();
        resources.insert(Resource {
            kind: "sql_connection",
            id: connection_id.clone(),
        });
        for (index, table) in query.tables.iter().enumerate() {
            let lock = format!(
                "LOCK TABLE ONLY {}.{} IN ACCESS SHARE MODE",
                table.schema.quoted(),
                table.name.quoted()
            );
            sqlx::query(AssertSqlSafe(lock)).execute(&mut *tx).await?;
            let relation = sqlx::query("SELECT c.oid::bigint AS oid FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace JOIN pg_catalog.pg_am a ON a.oid=c.relam WHERE n.nspname=$1 AND c.relname=$2 AND EXISTS (SELECT 1 FROM pg_catalog.pg_locks l WHERE l.pid=pg_catalog.pg_backend_pid() AND l.locktype='relation' AND l.relation=c.oid AND l.mode='AccessShareLock' AND l.granted) AND c.relkind='r' AND a.amname='heap' AND a.oid=2 AND NOT c.relrowsecurity AND NOT c.relforcerowsecurity AND NOT c.relhasrules AND NOT c.relispartition AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_inherits i WHERE i.inhrelid=c.oid OR i.inhparent=c.oid) AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_index i WHERE i.indrelid=c.oid AND (i.indexprs IS NOT NULL OR i.indpred IS NOT NULL)) AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class ic ON ic.oid=i.indexrelid JOIN pg_catalog.pg_am ia ON ia.oid=ic.relam WHERE i.indrelid=c.oid AND ia.amname NOT IN ('btree','hash')) AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_index i, unnest(i.indcollation) co WHERE i.indrelid=c.oid AND co<>0 AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_collation cl WHERE cl.oid=co AND cl.collnamespace='pg_catalog'::regnamespace)) AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_index i, unnest(i.indclass) op WHERE i.indrelid=c.oid AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_opclass oc WHERE oc.oid=op AND oc.opcnamespace='pg_catalog'::regnamespace))")
                .bind(table.schema.as_str()).bind(table.name.as_str()).fetch_optional(&mut *tx).await?.ok_or(Error::UnsupportedObject)?;
            let oid: i64 = relation.try_get("oid")?;
            relations.push(Oid(u32::try_from(oid).map_err(|_| Error::UnsupportedObject)?));
            let table_id = format!("{}/{database}/{oid}", connection_id.as_str());
            resources.insert(Resource {
                kind: "sql_table",
                id: ObjectId::new(table_id.clone())?,
            });
            let columns = sqlx::query("SELECT a.attname,a.attnum,a.atttypid::bigint AS type_oid,a.attgenerated::text AS generated,COALESCE(n.nspname='pg_catalog',true) AS safe_collation FROM pg_catalog.pg_attribute a LEFT JOIN pg_catalog.pg_collation c ON c.oid=a.attcollation LEFT JOIN pg_catalog.pg_namespace n ON n.oid=c.collnamespace WHERE a.attrelid=$1::bigint::oid AND a.attnum>0 AND NOT a.attisdropped").bind(oid).fetch_all(&mut *tx).await?;
            for row in columns {
                let type_oid =
                    u32::try_from(row.try_get::<i64, _>("type_oid")?).map_err(|_| Error::UnsupportedObject)?;
                let generated: String = row.try_get("generated")?;
                let safe_collation: bool = row.try_get("safe_collation")?;
                // Reject the whole table: even unselected generated attributes can introduce planner dependencies.
                if !generated.is_empty() {
                    return Err(Error::UnsupportedObject);
                }
                let name: String = row.try_get("attname")?;
                let attnum: i16 = row.try_get("attnum")?;
                if !safe_collation || ![16, 20, 21, 23, 25, 1043].contains(&type_oid) {
                    return Err(Error::UnsupportedObject);
                }
                catalog.push(CatalogColumn {
                    table: index,
                    name,
                    attnum,
                    type_oid,
                });
            }
        }
        let mut used = BTreeSet::new();
        let mut projections = Vec::new();
        let mut sizes = Vec::new();
        for (column, alias) in &query.select {
            let rendered = resolve(column, &query, &catalog, &mut used)?;
            if catalog.iter().any(|c| {
                c.name == column.name.as_str()
                    && column
                        .qualifier
                        .as_ref()
                        .is_none_or(|q| query.tables[c.table].alias == *q)
                    && [25, 1043].contains(&c.type_oid)
            }) {
                sizes.push(format!(
                    "COALESCE(pg_catalog.octet_length({rendered}),0)::pg_catalog.int8"
                ));
            }
            projections.push((rendered, alias.as_ref().unwrap_or(&column.name).quoted()));
        }
        let predicates = query
            .predicates
            .iter()
            .map(|expr| render_expr(expr, &query, &catalog, &mut used, parameters))
            .collect::<Result<Vec<_>, _>>()?;
        let mut orders = Vec::new();
        for (column, direction) in &query.order {
            if column.qualifier.is_none()
                && query
                    .select
                    .iter()
                    .any(|(_, alias)| alias.as_ref() == Some(&column.name))
            {
                return Err(Error::UnsupportedSql);
            }
            orders.push(format!("{} {direction}", resolve(column, &query, &catalog, &mut used)?));
        }
        for (table, attnum) in used {
            let relation_oid = relations[table].0;
            resources.insert(Resource {
                kind: "sql_column",
                id: ObjectId::new(format!("{}/{database}/{relation_oid}/{attnum}", connection_id.as_str()))?,
            });
        }
        let overflow = if sizes.is_empty() {
            "false".to_owned()
        } else {
            format!("({}) > {}", sizes.join(" + "), MAX_BYTES / 8)
        };
        let projections = projections
            .into_iter()
            .map(|(value, label)| format!("CASE WHEN ({overflow}) THEN NULL ELSE {value} END AS {label}"))
            .collect::<Vec<_>>();
        let markers = query
            .tables
            .iter()
            .enumerate()
            .map(|(index, table)| format!("{}.tableoid AS __mcp_relation_{index}", table.alias.quoted()))
            .collect::<Vec<_>>();
        let mut statement = format!(
            "SELECT ({overflow}) AS __mcp_overflow, {}, {} FROM {}",
            projections.join(", "),
            markers.join(", "),
            query.from
        );
        if !predicates.is_empty() {
            statement.push_str(&format!(" WHERE {}", predicates.join(" AND ")));
        }
        if !orders.is_empty() {
            statement.push_str(&format!(" ORDER BY {}", orders.join(", ")));
        }
        statement.push_str(&format!(" LIMIT {}", query.limit.statement_count()));
        let mut declaration = sqlx::query(AssertSqlSafe(format!(
            "DECLARE mcp_query NO SCROLL CURSOR WITHOUT HOLD FOR {statement}"
        )));
        for parameter in parameters {
            declaration = match parameter.value() {
                crate::parameter::ParameterValue::Boolean(v) => declaration.bind(*v),
                crate::parameter::ParameterValue::Integer(v) => declaration.bind(*v),
                crate::parameter::ParameterValue::Text(v) => declaration.bind(v),
            };
        }
        declaration.execute(&mut *tx).await?;
        // A cursor retains its bound plan even if a schema name changes during authorization.
        let metadata = (&mut *tx as &mut PgConnection)
            .prepare("FETCH FORWARD 0 FROM mcp_query".into_sql_str())
            .await?;
        let visible_columns = query.select.len();
        verify_bindings(metadata.columns(), visible_columns, &relations)?;
        crate::telemetry::references(&resources, &self.user, &self.client, connection_id);
        let requests = resources
            .into_iter()
            .flat_map(|resource| {
                [
                    crate::PermissionRequest {
                        resource: resource.clone(),
                        subject_kind: "sql_user",
                        subject: self.user.clone(),
                    },
                    crate::PermissionRequest {
                        resource,
                        subject_kind: "sql_client",
                        subject: self.client.clone(),
                    },
                ]
            })
            .collect::<Vec<_>>();
        self.permissions.check_all(&requests).await?;
        let columns = metadata
            .columns()
            .iter()
            .skip(1)
            .take(visible_columns)
            .map(|c| c.name().to_owned())
            .collect();
        let mut stream = sqlx::query("FETCH FORWARD ALL FROM mcp_query").fetch(&mut *tx);
        let mut result = QueryResult {
            columns,
            rows: Vec::new(),
        };
        let mut bytes = serde_json::to_vec(&result).map_err(|_| Error::Limit)?.len();
        while let Some(row) = stream.try_next().await? {
            if row.try_get::<bool, _>(0)? {
                return Err(Error::Limit);
            }
            if result.rows.len() == query.limit.result_count() {
                return Err(Error::Limit);
            }

            let values = decode(&row, visible_columns)?;
            bytes += serde_json::to_vec(&values).map_err(|_| Error::Limit)?.len() + 1;
            if bytes > MAX_BYTES {
                return Err(Error::Limit);
            }
            result.rows.push(values);
        }
        drop(stream);
        tx.rollback().await?;
        result.check_size()?;
        Ok(result)
    }
}

#[cfg(test)]
#[path = "postgres_tests.rs"]
mod tests;
fn verify_bindings(columns: &[PgColumn], visible_columns: usize, relations: &[Oid]) -> Result<(), Error> {
    if columns.len() != 1 + visible_columns + relations.len() {
        return Err(Error::UnsupportedObject);
    }
    for (column, relation) in columns.iter().skip(1 + visible_columns).zip(relations) {
        if column.relation_id() != Some(*relation) || column.relation_attribute_no() != Some(-6) {
            return Err(Error::UnsupportedObject);
        }
    }
    Ok(())
}

fn resolve(
    column: &Column,
    query: &Query,
    catalog: &[CatalogColumn],
    used: &mut BTreeSet<(usize, i16)>,
) -> Result<String, Error> {
    let matches: Vec<_> = catalog
        .iter()
        .filter(|c| {
            c.name == column.name.as_str()
                && column
                    .qualifier
                    .as_ref()
                    .is_none_or(|q| query.tables[c.table].alias == *q)
        })
        .collect();
    if matches.len() != 1 {
        return Err(Error::UnsupportedObject);
    }
    let found = matches[0];
    used.insert((found.table, found.attnum));
    Ok(format!(
        "{}.{}",
        query.tables[found.table].alias.quoted(),
        column.name.quoted()
    ))
}
fn render_expr(
    expr: &Expr,
    query: &Query,
    catalog: &[CatalogColumn],
    used: &mut BTreeSet<(usize, i16)>,
    parameters: &[crate::Parameter],
) -> Result<String, Error> {
    match expr {
        Expr::Column(c) => resolve(c, query, catalog, used),
        Expr::Literal(value) => Ok(value.clone()),
        Expr::Parameter(number) => {
            let parameter = parameters.get(number - 1).ok_or(Error::UnsupportedSql)?;
            let kind = match parameter.value() {
                crate::parameter::ParameterValue::Boolean(_) => "bool",
                crate::parameter::ParameterValue::Integer(_) => "int8",
                crate::parameter::ParameterValue::Text(_) => "text",
            };
            Ok(format!("CAST(${number} AS pg_catalog.{kind})"))
        }
        Expr::Compare(left, operator, right) => Ok(format!(
            "({} OPERATOR(pg_catalog.{operator}) {})",
            render_expr(left, query, catalog, used, parameters)?,
            render_expr(right, query, catalog, used, parameters)?
        )),
        Expr::Boolean(args, operator) => Ok(format!(
            "({})",
            args.iter()
                .map(|a| render_expr(a, query, catalog, used, parameters))
                .collect::<Result<Vec<_>, _>>()?
                .join(&format!(" {operator} "))
        )),
    }
}
fn decode(row: &PgRow, visible_columns: usize) -> Result<Vec<serde_json::Value>, Error> {
    row.columns()
        .iter()
        .enumerate()
        .skip(1)
        .take(visible_columns)
        .map(|(index, column)| {
            match column.type_info().name() {
                "BOOL" => serde_json::to_value(row.try_get::<Option<bool>, _>(index)?),
                "INT2" => serde_json::to_value(row.try_get::<Option<i16>, _>(index)?),
                "INT4" => serde_json::to_value(row.try_get::<Option<i32>, _>(index)?),
                "INT8" => serde_json::to_value(row.try_get::<Option<i64>, _>(index)?),
                "TEXT" | "VARCHAR" => serde_json::to_value(row.try_get::<Option<String>, _>(index)?),
                _ => return Err(Error::UnsupportedObject),
            }
            .map_err(|_| Error::Limit)
        })
        .collect()
}
