//! The ADR#0058 module store, against a live JetStream and a real component.
//!
//! The unit tests cover the reference grammar and the directory source with
//! bytes that are not wasm. What only a server and a real component can show is
//! that a published object comes back byte-identical through chunking, and that
//! a host refuses a component whose descriptor is not the module it asked for.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use trogon_decider_nats::DuplicateWindow;
use trogon_decider_nats_server::constants::{DEFAULT_QUEUE_GROUP, DEFAULT_SUBJECT_PREFIX};
use trogon_decider_nats_server::{
    CommandEndpoint, DeciderHost, FileModuleSource, ModuleReference, ModuleSource, ModuleStore,
    ObjectStoreModuleSource, ObjectStoreModuleSourceError, ServerConfig, StartupError, SubjectPrefix,
};
use trogon_decider_runtime::AdmissionLimit;
use trogon_nats::test_support::JetStreamTestServer;

const BUCKET: &str = "MODULE_STORE_TEST";
const MODULE_REFERENCE: &str = "scheduler.schedules@0.1.0";

fn module_path() -> PathBuf {
    let relative = "../../../target/wasm32-unknown-unknown/release/trogon_schedules_decider.wasm";
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    assert!(
        path.exists(),
        "build trogon_schedules_decider.wasm for wasm32-unknown-unknown first (expected {})",
        path.display()
    );
    path
}

fn reference(value: &str) -> ModuleReference {
    value.parse().expect("a well-formed reference")
}

fn config(root: &Path, modules: Vec<ModuleReference>) -> ServerConfig {
    ServerConfig {
        endpoint: CommandEndpoint::new(SubjectPrefix::new(DEFAULT_SUBJECT_PREFIX).expect("the default is a token"))
            .expect("the default prefix yields a conformant subject"),
        queue_group: DEFAULT_QUEUE_GROUP.to_owned(),
        events_stream: "MODULE_STORE_TEST_EVENTS".to_owned(),
        snapshot_bucket: "MODULE_STORE_TEST_SNAPSHOTS".to_owned(),
        module_store: ModuleStore::Directory(root.to_path_buf()),
        modules,
        admission_limit: AdmissionLimit::try_new(4).expect("four is non-zero"),
        replay_limit: None,
        duplicate_window: DuplicateWindow::try_new(Duration::from_secs(120)).expect("two minutes is enforceable"),
    }
}

#[tokio::test]
async fn a_published_component_comes_back_byte_identical() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    js.create_object_store(async_nats::jetstream::object_store::Config {
        bucket: BUCKET.to_owned(),
        ..Default::default()
    })
    .await
    .expect("the bucket is created");

    let reference = reference(MODULE_REFERENCE);
    let published = std::fs::read(module_path()).expect("the component is readable");
    let store = js.get_object_store(BUCKET).await.expect("the bucket exists");
    store
        .put(reference.object_key().as_str(), &mut published.as_slice())
        .await
        .expect("the component publishes");

    let source = ObjectStoreModuleSource::open(&js, BUCKET)
        .await
        .expect("the bucket opens");

    assert_eq!(
        source.fetch(&reference).await.expect("the component fetches"),
        published,
        "a component reassembled from its chunks has to be the component that was published, or every \
         module the host loads is a different program than the one that passed the conformance gate"
    );
}

#[tokio::test]
async fn a_reference_nobody_published_names_the_bucket_it_searched() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    js.create_object_store(async_nats::jetstream::object_store::Config {
        bucket: BUCKET.to_owned(),
        ..Default::default()
    })
    .await
    .expect("the bucket is created");

    let source = ObjectStoreModuleSource::open(&js, BUCKET)
        .await
        .expect("the bucket opens");

    let error = source
        .fetch(&reference("billing.invoices@2.3.1"))
        .await
        .expect_err("nothing was published under that reference");

    let ObjectStoreModuleSourceError::Get { key, bucket, .. } = error else {
        panic!("a reference nobody published fails on the lookup, not the read")
    };
    assert_eq!(
        (key.as_str(), bucket.as_str()),
        ("billing.invoices/2.3.1", BUCKET),
        "an operator has to be told which key was looked for in which bucket"
    );
}

#[tokio::test]
async fn a_bucket_nobody_provisioned_is_not_created_on_the_way_past() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;

    assert!(
        ObjectStoreModuleSource::open(&js, "NEVER_PUBLISHED").await.is_err(),
        "opening does not provision"
    );

    assert!(
        js.get_object_store("NEVER_PUBLISHED").await.is_err(),
        "a store that creates its own bucket turns 'the publisher never ran' into 'the module is missing \
         from a bucket that exists', which reads like a typo in a reference rather than a step nobody performed"
    );
}

#[tokio::test]
async fn a_component_that_is_not_what_was_asked_for_stops_startup() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;

    let root = tempfile::tempdir().expect("a temp dir");
    let requested = reference("scheduler.schedules@9.9.9");
    std::fs::copy(module_path(), root.path().join(requested.file_name())).expect("the component copies");

    let Err(error) = DeciderHost::start(
        &config(root.path(), vec![requested.clone()]),
        &FileModuleSource::new(root.path()),
        js,
    )
    .await
    else {
        panic!("the component declares 0.1.0, not 9.9.9");
    };

    assert!(
        matches!(
            error,
            StartupError::ModuleIdentityMismatch { ref reference, ref declared }
                if reference == &requested && declared == &self::reference(MODULE_REFERENCE)
        ),
        "the key a component was stored under is a claim the publisher made; the descriptor is what the \
         guest will behave as, and serving one module's commands under another module's name is worse \
         than not starting: {error}"
    );
}
