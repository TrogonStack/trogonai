use std::{error::Error as StdError, time::Duration};

use authzed::v1::{
    ContextualizedCaveat, ObjectReference, Relationship, RelationshipUpdate, SubjectReference,
    WriteRelationshipsRequest, WriteSchemaRequest, permissions_service_client::PermissionsServiceClient,
    relationship_update::Operation, schema_service_client::SchemaServiceClient,
};
use mcp_postgres::{Error, ObjectId, PermissionCheck, Resource, SpiceDb};
use testcontainers_modules::testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};

const TOKEN: &str = "test-only-token";

fn authorized<T>(value: T) -> Result<tonic::Request<T>, Box<dyn StdError>> {
    let mut request = tonic::Request::new(value);
    request
        .metadata_mut()
        .insert("authorization", format!("Bearer {TOKEN}").parse()?);
    Ok(request)
}

fn relationship(kind: &str, id: &str, relation: &str, subject_kind: &str, subject: &str) -> RelationshipUpdate {
    RelationshipUpdate {
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
    }
}

#[tokio::test]
async fn actual_spicedb_enforces_schema_grants_denials_and_subject_types() -> Result<(), Box<dyn StdError>> {
    let container = GenericImage::new("authzed/spicedb", "v1.53.0")
        .with_exposed_port(50051.tcp())
        .with_wait_for(WaitFor::message_on_either_std("grpc server started serving"))
        .with_startup_timeout(Duration::from_secs(90))
        .with_cmd(["serve", "--datastore-engine", "memory", "--grpc-preshared-key", TOKEN])
        .start()
        .await?;
    let endpoint = format!(
        "http://{}:{}",
        container.get_host().await?,
        container.get_host_port_ipv4(50051.tcp()).await?
    );
    let channel = tonic::transport::Channel::from_shared(endpoint.clone())?
        .connect()
        .await?;
    let mut schema = SchemaServiceClient::new(channel.clone());
    schema
        .write_schema(authorized(WriteSchemaRequest {
            schema: include_str!("../sql/schema.zed").into(),
        })?)
        .await?;
    let mut writer = PermissionsServiceClient::new(channel);
    let adapter = SpiceDb::connect(endpoint.clone(), TOKEN.into()).await?;
    let alice = ObjectId::new("alice".into())?;
    let desktop = ObjectId::new("desktop".into())?;
    let stranger = ObjectId::new("stranger".into())?;

    for kind in ["sql_connection", "sql_table", "sql_column"] {
        let resource = Resource {
            kind,
            id: ObjectId::new("registered/123/456/1".into())?,
        };
        assert!(matches!(
            adapter.check(&resource, "sql_user", &alice).await,
            Err(Error::Denied)
        ));
        writer
            .write_relationships(authorized(WriteRelationshipsRequest {
                updates: vec![
                    relationship(kind, resource.id.as_str(), "reader", "sql_user", "alice"),
                    relationship(kind, resource.id.as_str(), "reader", "sql_client", "desktop"),
                ],
                ..Default::default()
            })?)
            .await?;
        adapter.check(&resource, "sql_user", &alice).await?;
        adapter.check(&resource, "sql_client", &desktop).await?;
        assert!(matches!(
            adapter.check(&resource, "sql_user", &stranger).await,
            Err(Error::Denied)
        ));
        assert!(matches!(
            adapter.check(&resource, "sql_client", &alice).await,
            Err(Error::Denied)
        ));
        assert!(matches!(
            adapter.check(&resource, "sql_user", &desktop).await,
            Err(Error::Denied)
        ));

        writer
            .write_relationships(authorized(WriteRelationshipsRequest {
                updates: vec![relationship(
                    kind,
                    resource.id.as_str(),
                    "forbidden",
                    "sql_user",
                    "alice",
                )],
                ..Default::default()
            })?)
            .await?;
        assert!(matches!(
            adapter.check(&resource, "sql_user", &alice).await,
            Err(Error::Denied)
        ));
        adapter.check(&resource, "sql_client", &desktop).await?;

        writer
            .write_relationships(authorized(WriteRelationshipsRequest {
                updates: vec![relationship(kind, resource.id.as_str(), "forbidden", "sql_client", "*")],
                ..Default::default()
            })?)
            .await?;
        assert!(matches!(
            adapter.check(&resource, "sql_client", &desktop).await,
            Err(Error::Denied)
        ));

        let other_connection = Resource {
            kind,
            id: ObjectId::new("other/123/456/1".into())?,
        };
        assert!(matches!(
            adapter.check(&other_connection, "sql_user", &alice).await,
            Err(Error::Denied)
        ));
    }

    let global = Resource {
        kind: "sql_column",
        id: ObjectId::new("registered/123/456/2".into())?,
    };
    writer
        .write_relationships(authorized(WriteRelationshipsRequest {
            updates: vec![
                relationship("sql_column", global.id.as_str(), "reader", "sql_user", "alice"),
                relationship("sql_column", global.id.as_str(), "reader", "sql_client", "desktop"),
                relationship("sql_column", global.id.as_str(), "forbidden", "sql_user", "*"),
                relationship("sql_column", global.id.as_str(), "forbidden", "sql_client", "*"),
            ],
            ..Default::default()
        })?)
        .await?;
    assert!(matches!(
        adapter.check(&global, "sql_user", &alice).await,
        Err(Error::Denied)
    ));
    assert!(matches!(
        adapter.check(&global, "sql_client", &desktop).await,
        Err(Error::Denied)
    ));

    let invalid = writer
        .write_relationships(authorized(WriteRelationshipsRequest {
            updates: vec![relationship("sql_column", "invalid", "reader", "sql_user", "*")],
            ..Default::default()
        })?)
        .await;
    assert!(
        invalid.is_err(),
        "reader wildcard must not be accepted by production schema"
    );

    let resource = Resource {
        kind: "sql_column",
        id: ObjectId::new("registered/123/456/1".into())?,
    };
    let unauthenticated = SpiceDb::connect(endpoint, "wrong-test-token".into()).await?;
    assert!(matches!(
        unauthenticated.check(&resource, "sql_user", &alice).await,
        Err(Error::Authorization(_))
    ));
    assert!(matches!(
        adapter.check(&resource, "missing_subject_type", &alice).await,
        Err(Error::Denied)
    ));

    let extended = format!(
        "{}\ncaveat request_approved(approved bool) {{ approved }}\ndefinition conditional_resource {{\n relation reader: sql_user with request_approved\n permission query = reader\n}}",
        include_str!("../sql/schema.zed")
    );
    schema
        .write_schema(authorized(WriteSchemaRequest { schema: extended })?)
        .await?;
    let mut caveated = relationship("conditional_resource", "conditional", "reader", "sql_user", "alice");
    if let Some(relationship) = caveated.relationship.as_mut() {
        relationship.optional_caveat = Some(ContextualizedCaveat {
            caveat_name: "request_approved".into(),
            context: None,
        });
    }
    writer
        .write_relationships(authorized(WriteRelationshipsRequest {
            updates: vec![caveated],
            ..Default::default()
        })?)
        .await?;
    let conditional = Resource {
        kind: "conditional_resource",
        id: ObjectId::new("conditional".into())?,
    };
    assert!(
        matches!(
            adapter.check(&conditional, "sql_user", &alice).await,
            Err(Error::Denied)
        ),
        "conditional permissions must fail closed"
    );
    Ok(())
}
