#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use authzed::v1::{
    ObjectReference, Relationship, RelationshipUpdate, SubjectReference, WriteRelationshipsRequest, WriteSchemaRequest,
    permissions_service_client::PermissionsServiceClient, relationship_update::Operation,
    schema_service_client::SchemaServiceClient,
};
use mcp_postgres::{
    ConfigInput, Error, Parameter, ParameterInput, PermissionCheck, PermissionRequest, PostgresMcp, QueryService,
    RuntimeConfig,
};
use rmcp::{
    ServiceExt,
    model::{CallToolRequest, CallToolRequestParams, ClientRequest},
    service::PeerRequestOptions,
};
use serde_json::json;
use sqlx::{Executor, PgPool};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::sync::Notify;

#[derive(Default)]
struct Permissions {
    denied: Mutex<BTreeSet<(String, String)>>,
    checked: Mutex<Vec<(String, String, String)>>,
}

#[async_trait]
impl PermissionCheck for Permissions {
    async fn check_all(&self, requests: &[PermissionRequest]) -> Result<(), Error> {
        let denied = self.denied.lock().unwrap();
        let mut checked = self.checked.lock().unwrap();
        for request in requests {
            checked.push((
                request.resource.kind.to_owned(),
                request.resource.id.as_str().to_owned(),
                request.subject.as_str().to_owned(),
            ));
            if denied.contains(&(request.resource.id.as_str().to_owned(), request.subject_kind.to_owned())) {
                return Err(Error::Denied);
            }
        }
        Ok(())
    }
}

struct Fixture {
    _container: ContainerAsync<Postgres>,
    admin: PgPool,
    dsn: String,
}

impl Fixture {
    async fn new() -> Self {
        let container = Postgres::default().with_tag("18.3-alpine").start().await.unwrap();
        let host = container.get_host().await.unwrap();
        let port = container.get_host_port_ipv4(5432).await.unwrap();
        let admin = PgPool::connect(&format!("postgres://postgres:postgres@{host}:{port}/postgres"))
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE ROLE query_role LOGIN PASSWORD 'query-only';
            CREATE TABLE public.customers (id integer, email text, active boolean);
            INSERT INTO public.customers VALUES (1, 'one@example.test', true), (2, 'two@example.test', false);
            CREATE TABLE public.orders (id integer, customer_id integer, note text);
            INSERT INTO public.orders VALUES (9, 1, 'one@example.test');
            GRANT USAGE ON SCHEMA public TO query_role;
            GRANT SELECT ON ALL TABLES IN SCHEMA public TO query_role;
            ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT ON TABLES TO query_role;",
        )
        .execute(&admin)
        .await
        .unwrap();
        Self {
            _container: container,
            admin,
            dsn: format!("postgres://query_role:query-only@{host}:{port}/postgres"),
        }
    }

    async fn install_guard(&self) {
        sqlx::raw_sql(include_str!("../sql/install-ddl-guard.sql"))
            .execute(&self.admin)
            .await
            .unwrap();
    }

    async fn service(&self, permissions: Arc<dyn PermissionCheck>) -> QueryService {
        let input: ConfigInput = toml::from_str(include_str!("../examples/config.toml")).unwrap();
        let config = RuntimeConfig::from_input(input, |key| match key {
            "MCP_POSTGRES_SPICEDB_TOKEN" => Some("test-only-token".into()),
            "MCP_POSTGRES_ANALYTICS_DSN" => Some(self.dsn.clone()),
            _ => None,
        })
        .unwrap();
        QueryService::connect(config, permissions).await.unwrap()
    }

    async fn column_id(&self, table: &str, column: &str) -> String {
        let (database, relation, attribute): (i64, i64, i16) = sqlx::query_as("SELECT d.oid::bigint,c.oid::bigint,a.attnum FROM pg_catalog.pg_database d,pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace JOIN pg_catalog.pg_attribute a ON a.attrelid=c.oid WHERE d.datname=current_database() AND n.nspname='public' AND c.relname=$1 AND a.attname=$2")
            .bind(table).bind(column).fetch_one(&self.admin).await.unwrap();
        format!("analytics-local-v1/{database}/{relation}/{attribute}")
    }
}

#[tokio::test]
async fn actual_postgres_returns_authorized_results_and_preserves_limits() {
    let fixture = Fixture::new().await;
    fixture.install_guard().await;
    let permissions = Arc::new(Permissions::default());
    let service = fixture.service(permissions.clone()).await;
    let result = service
        .query(
            "analytics-readonly",
            "SELECT c.id, c.active FROM public.customers c ORDER BY c.id LIMIT 1",
        )
        .await
        .unwrap();
    assert_eq!(result.columns, ["id", "active"]);
    assert_eq!(result.rows, vec![vec![json!(1), json!(true)]]);
    let empty = service
        .query("analytics-readonly", "SELECT id FROM public.customers LIMIT 0")
        .await
        .unwrap();
    assert_eq!(empty.columns, ["id"]);
    assert!(empty.rows.is_empty());
    let absent = service
        .query("analytics-readonly", "SELECT id FROM public.customers WHERE id = 999")
        .await
        .unwrap();
    assert_eq!(absent.columns, ["id"]);
    assert!(absent.rows.is_empty());
    let joined = service
        .query(
            "analytics-readonly",
            "SELECT c.id, o.id AS order_id FROM public.customers c INNER JOIN public.orders o ON c.id = o.customer_id",
        )
        .await
        .unwrap();
    assert_eq!(joined.rows, vec![vec![json!(1), json!(9)]]);
    let checks = permissions.checked.lock().unwrap();
    for kind in ["sql_connection", "sql_table", "sql_column"] {
        for subject in ["example-user", "example-client"] {
            assert!(checks.iter().any(|(k, _, s)| k == kind && s == subject));
        }
    }
}

#[tokio::test]
async fn forbidden_columns_block_projections_predicates_joins_and_ordering_for_either_subject() {
    let fixture = Fixture::new().await;
    fixture.install_guard().await;
    let permissions = Arc::new(Permissions::default());
    let email = fixture.column_id("customers", "email").await;
    let service = fixture.service(permissions.clone()).await;
    for subject in ["sql_user", "sql_client"] {
        permissions.denied.lock().unwrap().clear();
        permissions
            .denied
            .lock()
            .unwrap()
            .insert((email.clone(), subject.into()));
        for sql in [
            "SELECT email FROM public.customers",
            "SELECT id FROM public.customers WHERE email = 'one@example.test'",
            "SELECT c.id FROM public.customers c INNER JOIN public.orders o ON c.email = o.note",
            "SELECT id FROM public.customers ORDER BY email",
        ] {
            assert!(
                matches!(service.query("analytics-readonly", sql).await, Err(Error::Denied)),
                "{sql}"
            );
        }
        assert!(
            service
                .query("analytics-readonly", "SELECT id FROM public.customers")
                .await
                .is_ok()
        );
    }
}

#[tokio::test]
async fn missing_disabled_or_replaced_guard_fails_closed() {
    let fixture = Fixture::new().await;
    let service = fixture.service(Arc::new(Permissions::default())).await;
    let query = "SELECT id FROM public.customers";
    assert!(matches!(
        service.query("analytics-readonly", query).await,
        Err(Error::UnsupportedObject)
    ));
    fixture.install_guard().await;
    assert!(service.query("analytics-readonly", query).await.is_ok());
    fixture
        .admin
        .execute("ALTER EVENT TRIGGER mcp_postgres_ddl_guard DISABLE")
        .await
        .unwrap();
    assert!(matches!(
        service.query("analytics-readonly", query).await,
        Err(Error::UnsupportedObject)
    ));
    fixture
        .admin
        .execute("ALTER EVENT TRIGGER mcp_postgres_ddl_guard ENABLE ALWAYS")
        .await
        .unwrap();
    fixture.admin.execute("CREATE OR REPLACE FUNCTION mcp_postgres_guard.lock_ddl() RETURNS event_trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS 'BEGIN RETURN; END;'").await.unwrap();
    assert!(matches!(
        service.query("analytics-readonly", query).await,
        Err(Error::UnsupportedObject)
    ));
}

#[tokio::test]
async fn catalog_features_and_unsupported_sql_cannot_expose_hidden_dependencies() {
    let fixture = Fixture::new().await;
    fixture.install_guard().await;
    sqlx::raw_sql("CREATE TABLE public.rls_table (id integer); ALTER TABLE public.rls_table ENABLE ROW LEVEL SECURITY;
        CREATE TABLE public.generated_table (id integer, generated integer GENERATED ALWAYS AS (id+1) STORED);
        CREATE TABLE public.parent_table (id integer); CREATE TABLE public.child_table () INHERITS (public.parent_table);
        CREATE DOMAIN public.custom_text AS text; CREATE TABLE public.custom_type_table (id integer, secret public.custom_text);
        CREATE VIEW public.customer_view AS SELECT id FROM public.customers;
        CREATE TABLE public.partitioned_table (id integer) PARTITION BY RANGE (id);
        CREATE TABLE public.partition_child PARTITION OF public.partitioned_table FOR VALUES FROM (0) TO (10);
        CREATE TABLE public.index_expression_table (id integer); CREATE INDEX ON public.index_expression_table ((id+1));")
        .execute(&fixture.admin).await.unwrap();
    let service = fixture.service(Arc::new(Permissions::default())).await;
    for table in [
        "rls_table",
        "generated_table",
        "parent_table",
        "child_table",
        "custom_type_table",
        "customer_view",
        "partitioned_table",
        "partition_child",
        "index_expression_table",
    ] {
        assert!(
            service
                .query("analytics-readonly", &format!("SELECT id FROM public.{table}"))
                .await
                .is_err(),
            "{table}"
        );
    }
    for sql in [
        "SELECT * FROM public.customers",
        "SELECT c.* FROM public.customers c",
        "SELECT c FROM public.customers c",
        "SELECT ctid FROM public.customers",
        "SELECT tableoid FROM public.customers",
        "SELECT id::text FROM public.customers",
        "SELECT length(email) FROM public.customers",
        "SELECT id FROM public.customers WHERE email OPERATOR(public.=) 'x'",
        "SELECT id FROM public.customers FOR UPDATE",
        "SELECT id INTO public.copied FROM public.customers",
        "WITH c AS (SELECT id FROM public.customers) SELECT id FROM c",
        "SELECT id FROM public.customers; SELECT email FROM public.customers",
        "SELECT id FROM public.customers WHERE id IN (SELECT id FROM public.orders)",
    ] {
        assert!(service.query("analytics-readonly", sql).await.is_err(), "{sql}");
    }
}

struct PausingPermissions {
    entered: Notify,
    release: Notify,
}

#[async_trait]
impl PermissionCheck for PausingPermissions {
    async fn check_all(&self, requests: &[PermissionRequest]) -> Result<(), Error> {
        for request in requests {
            if request.resource.kind == "sql_column" {
                self.entered.notify_one();
                self.release.notified().await;
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn ddl_guard_blocks_schema_rename_through_authorization_and_execution() {
    let fixture = Fixture::new().await;
    fixture.install_guard().await;
    let gate = Arc::new(PausingPermissions {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let service = Arc::new(fixture.service(gate.clone()).await);
    let task = tokio::spawn({
        let service = service.clone();
        async move {
            service
                .query("analytics-readonly", "SELECT id FROM public.customers")
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(3), gate.entered.notified())
        .await
        .unwrap();
    let mut migration = fixture.admin.acquire().await.unwrap();
    migration.execute("SET lock_timeout='150ms'").await.unwrap();
    let error = migration
        .execute("ALTER SCHEMA public RENAME TO moved")
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("55P03")
    );
    gate.release.notify_one();
    tokio::time::timeout(Duration::from_secs(3), gate.entered.notified())
        .await
        .unwrap();
    gate.release.notify_one();
    assert_eq!(task.await.unwrap().unwrap().rows.len(), 2);
    migration.execute("ALTER SCHEMA public RENAME TO moved").await.unwrap();
}

#[tokio::test]
async fn resource_limits_abort_without_returning_partial_results() {
    let fixture = Fixture::new().await;
    fixture.install_guard().await;
    sqlx::raw_sql(
        "CREATE TABLE public.many_rows (id integer); INSERT INTO public.many_rows SELECT generate_series(1,1001);
        CREATE TABLE public.large_rows (content text); INSERT INTO public.large_rows VALUES (repeat('x',1048577));",
    )
    .execute(&fixture.admin)
    .await
    .unwrap();
    let service = fixture.service(Arc::new(Permissions::default())).await;
    assert!(matches!(
        service
            .query("analytics-readonly", "SELECT id FROM public.many_rows")
            .await,
        Err(Error::Limit)
    ));
    assert!(matches!(
        service
            .query("analytics-readonly", "SELECT content FROM public.large_rows")
            .await,
        Err(Error::Limit)
    ));
    assert_eq!(
        service
            .query("analytics-readonly", "SELECT id FROM public.customers LIMIT 1")
            .await
            .unwrap()
            .rows
            .len(),
        1
    );
}

#[tokio::test]
async fn bound_parameters_keep_sql_looking_text_as_data_and_handle_integer_widths() {
    let fixture = Fixture::new().await;
    fixture.install_guard().await;
    fixture
        .admin
        .execute("CREATE TABLE public.widths (small smallint, regular integer, large bigint)")
        .await
        .unwrap();
    fixture
        .admin
        .execute("INSERT INTO public.widths VALUES (7,7,7)")
        .await
        .unwrap();
    let service = fixture.service(Arc::new(Permissions::default())).await;
    for column in ["small", "regular", "large"] {
        let parameters = [Parameter::try_from(ParameterInput::Integer(7)).unwrap()];
        let result = service
            .query_with_parameters(
                "analytics-readonly",
                &format!("SELECT {column} FROM public.widths WHERE {column} = $1"),
                &parameters,
            )
            .await
            .unwrap();
        assert_eq!(result.rows, vec![vec![json!(7)]]);
    }
    let parameters = [Parameter::try_from(ParameterInput::Text("' OR true --".into())).unwrap()];
    let result = service
        .query_with_parameters(
            "analytics-readonly",
            "SELECT id FROM public.customers WHERE email = $1",
            &parameters,
        )
        .await
        .unwrap();
    assert!(result.rows.is_empty());
    let parameters = [Parameter::try_from(ParameterInput::Boolean(true)).unwrap()];
    let result = service
        .query_with_parameters(
            "analytics-readonly",
            "SELECT id FROM public.customers WHERE active = $1",
            &parameters,
        )
        .await
        .unwrap();
    assert_eq!(result.rows, vec![vec![json!(1)]]);
    assert!(
        service
            .query("analytics-readonly", "SELECT id FROM public.customers WHERE id = $1")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn mcp_client_initializes_discovers_queries_and_cannot_spoof_identity() {
    let fixture = Fixture::new().await;
    fixture.install_guard().await;
    let service = fixture.service(Arc::new(Permissions::default())).await;
    let (client_io, server_io) = tokio::io::duplex(65536);
    let server_task = tokio::spawn(async move {
        PostgresMcp::new(service)
            .serve(server_io)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let client = ().serve(client_io).await.unwrap();
    let tools = client.list_tools(None).await.unwrap();
    assert_eq!(tools.tools.len(), 1);
    assert_eq!(tools.tools[0].name, "query");
    let result = client
        .call_tool(
            CallToolRequestParams::new("query").with_arguments(
                json!({"connection":"analytics-readonly","sql":"SELECT id FROM public.customers LIMIT 1"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_ne!(result.is_error, Some(true));
    let spoofed = client.call_tool(CallToolRequestParams::new("query").with_arguments(json!({"connection":"analytics-readonly","sql":"SELECT id FROM public.customers","user":"administrator"}).as_object().unwrap().clone())).await;
    assert!(spoofed.is_err());
    let invalid = client
        .call_tool(
            CallToolRequestParams::new("query").with_arguments(
                json!({"connection":"analytics-readonly","sql":"SELECT secret-marker-839 '"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(invalid.is_error, Some(true));
    assert!(!serde_json::to_string(&invalid).unwrap().contains("secret-marker-839"));
    client.cancel().await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), server_task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn cancelling_an_mcp_request_releases_its_database_guard() {
    let fixture = Fixture::new().await;
    fixture.install_guard().await;
    let gate = Arc::new(PausingPermissions {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let service = fixture.service(gate.clone()).await;
    let (client_io, server_io) = tokio::io::duplex(65536);
    let server = tokio::spawn(async move {
        PostgresMcp::new(service)
            .serve(server_io)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let client = ().serve(client_io).await.unwrap();
    let params = CallToolRequestParams::new("query").with_arguments(
        json!({"connection":"analytics-readonly","sql":"SELECT id FROM public.customers"})
            .as_object()
            .unwrap()
            .clone(),
    );
    let pending = client
        .peer()
        .send_request_with_option(
            ClientRequest::CallToolRequest(CallToolRequest::new(params)),
            PeerRequestOptions::no_options(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), gate.entered.notified())
        .await
        .unwrap();
    let unlocked: bool = sqlx::query_scalar("SELECT pg_catalog.pg_try_advisory_xact_lock(1879445091,1347635021)")
        .fetch_one(&fixture.admin)
        .await
        .unwrap();
    assert!(!unlocked);
    pending.cancel(Some("test cancellation".into())).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let unlocked: bool =
                sqlx::query_scalar("SELECT pg_catalog.pg_try_advisory_xact_lock(1879445091,1347635021)")
                    .fetch_one(&fixture.admin)
                    .await
                    .unwrap();
            if unlocked {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    client.cancel().await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}

fn spice_request<T>(value: T) -> tonic::Request<T> {
    let mut request = tonic::Request::new(value);
    request
        .metadata_mut()
        .insert("authorization", "Bearer test-only-token".parse().unwrap());
    request
}

#[tokio::test]
async fn mcp_spicedb_and_postgres_enforce_hard_column_denial_together() {
    let fixture = Fixture::new().await;
    fixture.install_guard().await;
    let spice = GenericImage::new("authzed/spicedb", "v1.53.0")
        .with_exposed_port(50051.tcp())
        .with_wait_for(WaitFor::message_on_either_std("grpc server started serving"))
        .with_startup_timeout(Duration::from_secs(90))
        .with_cmd([
            "serve",
            "--datastore-engine",
            "memory",
            "--grpc-preshared-key",
            "test-only-token",
        ])
        .start()
        .await
        .unwrap();
    let endpoint = format!(
        "http://{}:{}",
        spice.get_host().await.unwrap(),
        spice.get_host_port_ipv4(50051.tcp()).await.unwrap()
    );
    let channel = tonic::transport::Channel::from_shared(endpoint.clone())
        .unwrap()
        .connect()
        .await
        .unwrap();
    SchemaServiceClient::new(channel.clone())
        .write_schema(spice_request(WriteSchemaRequest {
            schema: include_str!("../sql/schema.zed").into(),
        }))
        .await
        .unwrap();
    let mut writer = PermissionsServiceClient::new(channel);
    let id_column = fixture.column_id("customers", "id").await;
    let email_column = fixture.column_id("customers", "email").await;
    let table = id_column.rsplit_once('/').unwrap().0;
    let update = |kind: &str, id: &str, relation: &str, subject_kind: &str, subject: &str| RelationshipUpdate {
        operation: Operation::Touch as i32,
        relationship: Some(Relationship {
            resource: Some(ObjectReference {
                object_type: kind.into(),
                object_id: id.into(),
            }),
            relation: relation.into(),
            subject: Some(SubjectReference {
                object: Some(ObjectReference {
                    object_type: subject_kind.into(),
                    object_id: subject.into(),
                }),
                optional_relation: String::new(),
            }),
            ..Default::default()
        }),
    };
    let mut grants = Vec::new();
    for (kind, id) in [
        ("sql_connection", "analytics-local-v1"),
        ("sql_table", table),
        ("sql_column", id_column.as_str()),
        ("sql_column", email_column.as_str()),
    ] {
        grants.push(update(kind, id, "reader", "sql_user", "example-user"));
        grants.push(update(kind, id, "reader", "sql_client", "example-client"));
    }
    writer
        .write_relationships(spice_request(WriteRelationshipsRequest {
            updates: grants,
            ..Default::default()
        }))
        .await
        .unwrap();
    let adapter = Arc::new(
        mcp_postgres::SpiceDb::connect(endpoint, "test-only-token".into())
            .await
            .unwrap(),
    );
    let service = fixture.service(adapter).await;
    let (client_io, server_io) = tokio::io::duplex(65536);
    let server = tokio::spawn(async move {
        PostgresMcp::new(service)
            .serve(server_io)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let client = ().serve(client_io).await.unwrap();
    let query = |sql: &str| {
        CallToolRequestParams::new("query").with_arguments(
            json!({"connection":"analytics-readonly","sql":sql})
                .as_object()
                .unwrap()
                .clone(),
        )
    };
    let allowed = client
        .call_tool(query("SELECT email FROM public.customers ORDER BY id"))
        .await
        .unwrap();
    assert_ne!(allowed.is_error, Some(true));
    let wire = serde_json::to_value(&allowed).unwrap();
    let data: serde_json::Value = serde_json::from_str(wire["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(data["rows"][0][0], "one@example.test");
    writer
        .write_relationships(spice_request(WriteRelationshipsRequest {
            updates: vec![update("sql_column", &email_column, "forbidden", "sql_client", "*")],
            ..Default::default()
        }))
        .await
        .unwrap();
    for sql in [
        "SELECT email FROM public.customers",
        "SELECT id FROM public.customers WHERE email = 'one@example.test'",
    ] {
        let denied = client.call_tool(query(sql)).await.unwrap();
        assert_eq!(denied.is_error, Some(true));
        let wire = serde_json::to_string(&denied).unwrap();
        assert!(wire.contains("access denied"));
        assert!(!wire.contains("one@example.test"));
    }
    let still_allowed = client
        .call_tool(query("SELECT id FROM public.customers"))
        .await
        .unwrap();
    assert_ne!(still_allowed.is_error, Some(true));
    client.cancel().await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}
