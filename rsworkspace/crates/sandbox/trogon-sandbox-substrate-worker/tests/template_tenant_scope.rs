//! A template is scoped to the tenant's atespace: identical workloads in two tenants build
//! two native templates, and a cached `Ready` record never vouches for a template that is
//! gone. Runs against a real JetStream KV index.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::SubstrateSizeClasses;
use trogon_sandbox_substrate_worker::proto;
use trogon_sandbox_substrate_worker::{
    ActorResources, AteError, AteResult, Control, NatsTemplateIndex, ResourceRevision, SandboxClass, SubstrateConfig,
    TemplateError, TemplateKey, TemplateRegistry,
};

type TestResult = Result<(), Box<dyn Error>>;

const IMAGE: &str = "registry.example/agent@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[derive(Default)]
struct NativeTemplates {
    templates: Mutex<HashMap<(String, String), proto::ActorTemplate>>,
    creates: Mutex<Vec<proto::ObjectRef>>,
}

impl NativeTemplates {
    async fn remove(&self, reference: &proto::ObjectRef) {
        self.templates
            .lock()
            .await
            .remove(&(reference.atespace.clone(), reference.name.clone()));
    }

    async fn creates_in(&self, atespace: &str) -> usize {
        self.creates
            .lock()
            .await
            .iter()
            .filter(|item| item.atespace == atespace)
            .count()
    }
}

fn outside_the_template_surface<T>() -> AteResult<T> {
    Err(AteError::Other {
        message: "template readiness does not use this call".into(),
    })
}

#[async_trait]
impl Control for NativeTemplates {
    async fn create_atespace(&self, _: &str) -> AteResult<proto::Atespace> {
        outside_the_template_surface()
    }

    async fn get_actor(&self, _: proto::ObjectRef) -> AteResult<proto::Actor> {
        outside_the_template_surface()
    }

    async fn create_actor(&self, _: proto::Actor) -> AteResult<proto::Actor> {
        outside_the_template_surface()
    }

    async fn resume_actor(&self, _: proto::ObjectRef) -> AteResult<proto::Actor> {
        outside_the_template_surface()
    }

    async fn suspend_actor(&self, _: proto::ObjectRef) -> AteResult<proto::Actor> {
        outside_the_template_surface()
    }

    async fn delete_actor(&self, _: ResourceRevision) -> AteResult<proto::Actor> {
        outside_the_template_surface()
    }

    async fn list_actors(&self, _: &str) -> AteResult<Vec<proto::Actor>> {
        outside_the_template_surface()
    }

    async fn create_actor_template(&self, mut template: proto::ActorTemplate) -> AteResult<proto::ActorTemplate> {
        let metadata = template.metadata.as_mut().ok_or_else(|| AteError::InvalidArgument {
            message: "template metadata is missing".into(),
        })?;
        let reference = proto::ObjectRef {
            atespace: metadata.atespace.clone(),
            name: metadata.name.clone(),
        };
        let mut templates = self.templates.lock().await;
        let Entry::Vacant(entry) = templates.entry((reference.atespace.clone(), reference.name.clone())) else {
            return Err(AteError::AlreadyExists {
                message: "native template already exists".into(),
            });
        };
        let mut creates = self.creates.lock().await;
        metadata.uid = format!("00000000-0000-4000-8000-{:012}", creates.len() + 1);
        metadata.version = 1;
        template.status = Some(proto::ActorTemplateStatus {
            golden_snapshot_status: Some(proto::GoldenSnapshotStatus {
                golden_tag: Some(proto::ObjectRef {
                    atespace: reference.atespace.clone(),
                    name: format!("golden-{}", reference.name),
                }),
                ..Default::default()
            }),
        });
        creates.push(reference);
        entry.insert(template.clone());
        Ok(template)
    }

    async fn get_actor_template(&self, reference: proto::ObjectRef) -> AteResult<proto::ActorTemplate> {
        self.templates
            .lock()
            .await
            .get(&(reference.atespace, reference.name))
            .cloned()
            .ok_or_else(|| AteError::NotFound {
                message: "native template is absent".into(),
            })
    }

    async fn delete_actor_template(&self, revision: ResourceRevision) -> AteResult<()> {
        let reference = revision.resource();
        let key = (reference.atespace.clone(), reference.name.clone());
        let mut templates = self.templates.lock().await;
        let existing = templates.get(&key).ok_or_else(|| AteError::NotFound {
            message: "native template is absent".into(),
        })?;
        let expected = revision.options();
        let current = existing
            .metadata
            .as_ref()
            .map(|metadata| (&metadata.uid, metadata.version));
        if current != Some((&expected.uid, expected.version)) {
            return Err(AteError::FailedPrecondition {
                message: "native template revision changed".into(),
            });
        }
        templates.remove(&key);
        Ok(())
    }
}

fn config() -> Result<SubstrateConfig, Box<dyn Error>> {
    Ok(SubstrateConfig {
        ate_api_endpoint: "http://127.0.0.1:8080".into(),
        router_endpoint: "127.0.0.1:8081".parse()?,
        router_tls: None,
        actor_cpu_millicores: 1000,
        agent_image: IMAGE.into(),
        agent_port: 8110,
        sandbox_class: SandboxClass::Gvisor,
        snapshots_location: String::new(),
        size_classes: "small=2048,medium=8192,large=32768".parse::<SubstrateSizeClasses>()?,
        template_build_timeout: Duration::from_secs(1),
        resume_timeout: Duration::from_secs(1),
        max_concurrent_leases: 4,
    })
}

fn key(config: &SubstrateConfig) -> Result<TemplateKey, Box<dyn Error>> {
    Ok(TemplateKey::new(
        IMAGE,
        SandboxClass::Gvisor,
        config,
        "small",
        ActorResources::new(1000, 2048)?,
    ))
}

async fn registry(server: &JetStreamTestServer) -> Result<TemplateRegistry, Box<dyn Error>> {
    let index = NatsTemplateIndex::new(server.jetstream().await).await?;
    Ok(TemplateRegistry::new(Arc::new(index)).with_poll_interval(Duration::from_millis(1)))
}

#[tokio::test]
async fn identical_workloads_build_native_templates_in_each_tenant() -> TestResult {
    let config = config()?;
    let key = key(&config)?;
    let server = JetStreamTestServer::start().await;
    let registry = registry(&server).await?;
    let control = NativeTemplates::default();

    for tenant in ["tenant-a", "tenant-b"] {
        let reference = registry
            .ensure_ready(&control, &config, tenant, &key, Duration::from_secs(1))
            .await?;
        assert_eq!(reference.atespace, tenant);
        assert!(
            control.get_actor_template(reference).await.is_ok(),
            "readiness must refer to a native template in {tenant}"
        );
        assert_eq!(control.creates_in(tenant).await, 1);
    }
    Ok(())
}

#[tokio::test]
async fn cached_readiness_does_not_hide_a_missing_native_template() -> TestResult {
    let config = config()?;
    let key = key(&config)?;
    let server = JetStreamTestServer::start().await;
    let registry = registry(&server).await?;
    let control = NativeTemplates::default();
    let original = registry
        .ensure_ready(&control, &config, "tenant-a", &key, Duration::from_secs(1))
        .await?;
    assert!(control.get_actor_template(original.clone()).await.is_ok());
    control.remove(&original).await;

    let error = registry
        .ensure_ready(&control, &config, "tenant-a", &key, Duration::from_millis(100))
        .await
        .err();

    assert!(
        matches!(error, Some(TemplateError::NotReady { .. })),
        "a cached READY record must not report readiness for an absent native template: {error:?}"
    );
    assert_eq!(
        control.creates_in("tenant-a").await,
        1,
        "the cached record means no rebuild"
    );
    Ok(())
}
