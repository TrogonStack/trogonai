//! The secrets service binary run from its command line: `config dump
//! --resolved` prints the configuration without connecting to anything, and
//! without a command the binary starts the service as it always has.

#![allow(clippy::expect_used, clippy::panic)]

use std::io::ErrorKind;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use nkeys::{KeyPair, XKey};

/// What the sealing key seed variable holds when the dump runs. The dump never
/// reads it, so it need not be a seed, and no output may show it.
const FAKE_SEED: &str = "fake-xkey-seed-content";
const FAKE_NATS_PASSWORD: &str = "fake-nats-password";
const RELEASE_POLICIES: &str = r#"
[[policies]]
owner_id = "project-1"
credential_id = "0190a5b4-7c1e-7d2a-9f3b-6c8d4e2a1b0f"
runtime_services = ["billing-runner"]
hosts = ["api.github.com"]
headers = ["authorization"]
"#;
const DEADLINE: Duration = Duration::from_secs(15);

/// How a run of the binary ended.
enum Outcome {
    Exited(Output),
    ConnectedToNats,
}

/// A directory for the files one test points the service at, removed when the
/// test ends.
struct FilesDirectory(PathBuf);

impl FilesDirectory {
    fn create(test: &str) -> Self {
        let path = std::env::temp_dir().join(format!("trogon-secrets-service-{test}-{}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create the files directory");
        Self(path)
    }
}

impl Drop for FilesDirectory {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("could not remove {}: {error}", self.0.display());
        }
    }
}

/// A port nothing answers on, so a test sees every connection made to it.
fn observer() -> TcpListener {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind an observer");
    listener.set_nonblocking(true).expect("nonblocking observer");
    listener
}

fn port(listener: &TcpListener) -> u16 {
    listener.local_addr().expect("observer address").port()
}

/// The binary with every setting it needs in its environment and nothing else:
/// NATS and OpenBao at the two observers, the files it reads in `files`, and
/// the sealing key seed read from `FAKE_XKEY_SEED`.
fn service(nats: &TcpListener, openbao: &TcpListener, files: &Path, seed: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_trogon-secrets-service"));
    command
        .env_clear()
        .env("TOKIO_WORKER_THREADS", "2")
        .env("NATS_URL", format!("nats://127.0.0.1:{}", port(nats)))
        .env("NATS_TLS_DISABLED", "yes")
        .env("OPENBAO_ADDR", format!("http://127.0.0.1:{}", port(openbao)))
        .env("OPENBAO_TLS_DISABLED", "1")
        .env("OPENBAO_MOUNT", "secret")
        .env("OPENBAO_ROLE_ID", "fake-role-id")
        .env("OPENBAO_SECRET_ID_FILE", files.join("secret-id"))
        .env("OPENBAO_CHILD_TOKEN_ROLE", "secrets-release")
        .env("OPENBAO_CHILD_TOKEN_POLICY", "secrets-release-read")
        .env("TROGON_SECRETS_TENANT_OWNERS_FILE", files.join("tenant-owners.toml"))
        .env(
            "TROGON_SECRETS_RELEASE_POLICIES_FILE",
            files.join("release-policies.toml"),
        )
        .env("TROGON_SECRETS_XKEY_SEED", "env://FAKE_XKEY_SEED")
        .env("FAKE_XKEY_SEED", seed)
        .env("TROGON_SECRETS_AUDIT_TENANT", "acme")
        .env("TROGON_SECRETS_AUDIT_PROJECT", "platform")
        .env("TROGON_SECRETS_AUDIT_INSTANCE", "secrets-0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Runs `command` until it exits or connects to `nats`, whichever comes first.
fn run(command: &mut Command, nats: &TcpListener) -> Outcome {
    let mut child = command.spawn().expect("start the secrets service binary");
    let deadline = Instant::now() + DEADLINE;
    loop {
        match nats.accept() {
            Ok(_) => {
                child.kill().expect("stop the connected service");
                child.wait_with_output().expect("reap the connected service");
                return Outcome::ConnectedToNats;
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => panic!("observe the NATS connection: {error}"),
        }
        if child.try_wait().expect("inspect the service process").is_some() {
            return Outcome::Exited(child.wait_with_output().expect("capture the service output"));
        }
        if Instant::now() >= deadline {
            child.kill().expect("stop the stalled service");
            child.wait_with_output().expect("reap the stalled service");
            panic!("the service neither exited nor connected before its deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn exited(outcome: Outcome) -> Output {
    match outcome {
        Outcome::Exited(output) => output,
        Outcome::ConnectedToNats => panic!("the service connected to NATS"),
    }
}

fn assert_never_connected(listener: &TcpListener, to: &str) {
    match listener.accept() {
        Err(error) if error.kind() == ErrorKind::WouldBlock => {}
        Ok(_) => panic!("the service connected to {to}"),
        Err(error) => panic!("observe the {to} connection: {error}"),
    }
}

/// The log line without the timestamp it starts with, which differs between
/// two runs.
fn without_timestamp(stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    stderr
        .lines()
        .map(|line| line.split_once(' ').map_or(line, |(_, rest)| rest))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn config_dump_prints_every_setting_without_connecting_or_reading_a_secret() {
    let (nats, openbao) = (observer(), observer());
    // Nothing is written here: the dump reads none of the files it names.
    let files = std::env::temp_dir().join(format!("trogon-secrets-service-absent-{}", std::process::id()));
    let mut command = service(&nats, &openbao, &files, FAKE_SEED);
    command
        .env(
            "NATS_URL",
            format!("nats://fake-user:{FAKE_NATS_PASSWORD}@127.0.0.1:{}", port(&nats)),
        )
        .args(["config", "dump", "--resolved"]);

    let output = exited(run(&mut command, &nats));

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let files = files.display();
    assert_eq!(
        stdout,
        format!(
            "\
nats.url (NATS_URL) = nats://<redacted>@127.0.0.1:{nats_port} (from environment)
nats.creds (NATS_CREDS) = <unset> (from default)
nats.nkey (NATS_NKEY) = <unset> (from default)
nats.user (NATS_USER) = <unset> (from default)
nats.password (NATS_PASSWORD) = <unset> (from default)
nats.token (NATS_TOKEN) = <unset> (from default)
nats.jwt (NATS_JWT) = <unset> (from default)
nats.seed (NATS_SEED) = <unset> (from default)
nats.tls_cert (NATS_CERT) = <unset> (from default)
nats.tls_key (NATS_KEY) = <unset> (from default)
nats.tls_ca (NATS_CA) = <unset> (from default)
NATS_TLS_DISABLED = true (from environment)
openbao.address (OPENBAO_ADDR) = http://127.0.0.1:{openbao_port} (from environment)
OPENBAO_TLS_DISABLED = true (from environment)
openbao.mount (OPENBAO_MOUNT) = secret (from environment)
openbao.role_id (OPENBAO_ROLE_ID) = fake-role-id (from environment)
openbao.secret_id_file (OPENBAO_SECRET_ID_FILE) = {files}/secret-id (from environment)
openbao.child_token_role (OPENBAO_CHILD_TOKEN_ROLE) = secrets-release (from environment)
openbao.child_token_policy (OPENBAO_CHILD_TOKEN_POLICY) = secrets-release-read (from environment)
release.tenant_owners_file (TROGON_SECRETS_TENANT_OWNERS_FILE) = {files}/tenant-owners.toml (from environment)
release.policies_file (TROGON_SECRETS_RELEASE_POLICIES_FILE) = {files}/release-policies.toml (from environment)
release.xkey_seed (TROGON_SECRETS_XKEY_SEED) = env://FAKE_XKEY_SEED (from environment)
release.credential_stream (TROGON_SECRETS_CREDENTIAL_STREAM) = CREDENTIAL_EVENTS (from default)
release.admission_limit (TROGON_SECRETS_ADMISSION_LIMIT) = 64 (from default)
release.invalidation_stream_replicas (TROGON_SECRETS_INVALIDATION_STREAM_REPLICAS) = 3 (from default)
audit.tenant (TROGON_SECRETS_AUDIT_TENANT) = acme (from environment)
audit.project (TROGON_SECRETS_AUDIT_PROJECT) = platform (from environment)
audit.instance (TROGON_SECRETS_AUDIT_INSTANCE) = secrets-0 (from environment)
",
            nats_port = port(&nats),
            openbao_port = port(&openbao),
        )
    );
    assert!(!stdout.contains(FAKE_SEED), "the dump printed the sealing key seed");
    assert!(
        !stdout.contains(FAKE_NATS_PASSWORD),
        "the dump printed the NATS password"
    );
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    assert_never_connected(&nats, "NATS");
    assert_never_connected(&openbao, "OpenBao");
}

#[test]
fn without_a_command_the_service_starts_and_connects_to_nats() {
    let (nats, openbao) = (observer(), observer());
    let files = FilesDirectory::create("serves");
    std::fs::write(
        files.0.join("tenant-owners.toml"),
        format!(
            "[tenants]\n\"{}\" = [\"project-1\"]\n",
            KeyPair::new_account().public_key()
        ),
    )
    .expect("write the tenant owners file");
    std::fs::write(files.0.join("release-policies.toml"), RELEASE_POLICIES).expect("write the release policies file");
    std::fs::write(files.0.join("secret-id"), "fake-secret-id\n").expect("write the secret id file");
    let seed = XKey::new().seed().expect("a sealing key seed");

    let outcome = run(&mut service(&nats, &openbao, &files.0, &seed), &nats);

    match outcome {
        Outcome::ConnectedToNats => {}
        Outcome::Exited(output) => panic!(
            "the service exited with {} instead of connecting to NATS: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ),
    }
}

#[test]
fn config_dump_refuses_a_configuration_the_way_startup_does() {
    let (nats, openbao) = (observer(), observer());
    let files = std::env::temp_dir().join(format!("trogon-secrets-service-refused-{}", std::process::id()));
    let refused = |command: &[&str]| {
        let mut service = service(&nats, &openbao, &files, FAKE_SEED);
        service.env("NATS_TLS_DISABLED", "maybe").args(command);
        exited(run(&mut service, &nats))
    };

    let dump = refused(&["config", "dump", "--resolved"]);
    let startup = refused(&[]);

    for output in [&dump, &startup] {
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    }
    let refusal = without_timestamp(&dump.stderr);
    assert!(
        refusal.contains("trogon-secrets-service stopped on an error"),
        "the dump refused without the startup error: {refusal}"
    );
    assert!(
        refusal.contains("NATS_TLS_DISABLED"),
        "the dump refused for another reason: {refusal}"
    );
    assert_eq!(refusal, without_timestamp(&startup.stderr));
    assert_never_connected(&nats, "NATS");
    assert_never_connected(&openbao, "OpenBao");
}
