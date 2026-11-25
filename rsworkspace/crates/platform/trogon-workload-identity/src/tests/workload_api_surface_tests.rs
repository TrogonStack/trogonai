use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::Duration;

use futures::StreamExt;

use crate::constants::TEST_TIMEOUT;
use crate::test_support::{TestCa, WorkloadApiFixture};
use crate::tests::LOCAL_ID;
use crate::workload_stream;

const SERVICE: &str = "/SpiffeWorkloadAPI/";

fn collect_methods(dir: &Path, methods: &mut BTreeSet<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.starts_with("test_support") || name.starts_with("tests") {
            continue;
        }
        if path.is_dir() {
            collect_methods(&path, methods);
            continue;
        }
        let source = fs::read_to_string(&path).unwrap();
        for (index, _) in source.match_indices(SERVICE) {
            let method: String = source[index + SERVICE.len()..]
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect();
            methods.insert(method);
        }
    }
}

#[test]
fn the_crate_fetches_only_x509_svids_and_never_requests_a_jwt_svid() {
    let mut methods = BTreeSet::new();
    collect_methods(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut methods);

    assert_eq!(
        methods,
        BTreeSet::from(["FetchJWTBundles".to_owned(), "FetchX509SVID".to_owned()])
    );
}

/// Transport headers that every gRPC-over-HTTP/2 call carries. Anything else
/// the client sends is application metadata and must be exactly the Workload
/// API marker: the caller presents no token and no credential.
const TRANSPORT_HEADERS: [&str; 5] = [
    "content-type",
    "te",
    "user-agent",
    "grpc-accept-encoding",
    "grpc-timeout",
];

#[tokio::test]
async fn the_workload_api_client_sends_only_the_workload_marker_and_no_credential() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    let mut x509 = workload_stream::connect(&fixture.endpoint()).await.unwrap();
    tokio::time::timeout(TEST_TIMEOUT, x509.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let mut jwt_bundles = workload_stream::connect_jwt_bundles(&fixture.endpoint()).await.unwrap();
    fixture.replace_jwt_bundles(&[("example.org", "{\"keys\":[]}")]);
    tokio::time::timeout(TEST_TIMEOUT, jwt_bundles.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    let application_headers: Vec<String> = fixture
        .request_header_names()
        .into_iter()
        .filter(|name| !name.starts_with(':') && !TRANSPORT_HEADERS.contains(&name.as_str()))
        .collect();
    assert_eq!(application_headers, vec!["workload.spiffe.io".to_owned()]);
    fixture.stop().await;
}

#[test]
fn the_x509_path_takes_no_audience() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for file in [
        "tls.rs",
        "tls_guards.rs",
        "certificate_validation.rs",
        "peer_evidence.rs",
    ] {
        let text = fs::read_to_string(source.join(file)).unwrap().to_ascii_lowercase();
        assert!(!text.contains("audience"), "{file} must not take an audience");
    }
}

#[test]
fn svids_are_consumed_through_the_spiffe_library_transport() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let dependencies = manifest.split("[dev-dependencies]").next().unwrap();
    assert!(
        dependencies.lines().any(|line| line.starts_with("spiffe")),
        "spiffe must be a runtime dependency"
    );
    let stream = fs::read_to_string(root.join("src/workload_stream.rs")).unwrap();
    assert!(stream.contains("spiffe::transport::connect"));
}

/// Key material stays in memory: the SVID private key arrives over the
/// Workload API and is handed to rustls, never persisted. No product source
/// may touch the filesystem or a temporary file.
const DISK_WRITE_MARKERS: [&str; 7] = [
    "std::fs",
    "tokio::fs",
    "fs::write",
    "File::create",
    "File::options",
    "OpenOptions",
    "tempfile",
];

fn scan_for_disk_writes(dir: &Path, hits: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.starts_with("test_support") || name.starts_with("tests") {
            continue;
        }
        if path.is_dir() {
            scan_for_disk_writes(&path, hits);
            continue;
        }
        let source = fs::read_to_string(&path).unwrap();
        for marker in DISK_WRITE_MARKERS {
            if source.contains(marker) {
                hits.push(format!("{name}: {marker}"));
            }
        }
    }
}

#[test]
fn product_source_never_writes_key_material_to_disk() {
    let mut hits = Vec::new();
    scan_for_disk_writes(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut hits);
    assert!(hits.is_empty(), "product source touches the filesystem: {hits:?}");
}
