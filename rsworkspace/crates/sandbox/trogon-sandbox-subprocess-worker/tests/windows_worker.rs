#![cfg(windows)]

use std::collections::BTreeMap;
use std::fs;
use std::net::TcpListener;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use trogon_sandbox_contract::{
    HostMountAccess, LeaseId, LeaseRequest, LeaseRequestKey, NetworkPolicy, OciImageRef, PrivilegedAccess,
    ProviderKind, SandboxImage, SandboxSpec, SandboxSpecParts, SandboxTags, TenantId, WorkspaceSource,
};
use trogon_sandbox_protocol::Provider as _;
use trogon_sandbox_subprocess_worker::{WindowsWorker, WindowsWorkerConfig};
use trogon_std::env::{ReadEnv as _, SystemEnv};
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};

type TestError = Box<dyn std::error::Error>;

struct Harness {
    root: tempfile::TempDir,
    workspace: PathBuf,
    worker: WindowsWorker,
}

impl Harness {
    fn new(script: &str) -> Result<Self, TestError> {
        let root = tempfile::tempdir()?;
        let workspace = root.path().join("workspace");
        fs::create_dir(&workspace)?;
        let worker = WindowsWorker::new(WindowsWorkerConfig {
            agent_binary_default: system32("cmd.exe")?,
            agent_args: ["/d", "/s", "/c", script].iter().map(|arg| arg.to_string()).collect(),
            workspace_root: root.path().to_path_buf(),
            egress_proxy_binary: None,
            fence_helper: None,
        })?;
        Ok(Self {
            root,
            workspace,
            worker,
        })
    }

    fn request(&self, edit: impl FnOnce(&mut SandboxSpecParts)) -> Result<LeaseRequest, TestError> {
        let mut parts = SandboxSpecParts {
            image: SandboxImage::Registry {
                reference: OciImageRef {
                    registry: String::new(),
                    repository: system32("cmd.exe")?.to_string_lossy().into_owned(),
                    tag: None,
                    digest: None,
                },
                pull_secret: None,
            },
            workspace: WorkspaceSource::HostMount {
                path: self.workspace.to_string_lossy().into_owned(),
                access: HostMountAccess::ReadWrite,
            },
            env: BTreeMap::new(),
            cpu_seconds: NonZeroU32::new(30).ok_or("zero cpu")?,
            memory_mib: NonZeroU32::new(512).ok_or("zero memory")?,
            snapshot_out: None,
            network_policy: None,
            mounts: Vec::new(),
            drives: Vec::new(),
            exposed_ports: Vec::new(),
            boot_mode: Default::default(),
            working_dir: None,
            user: None,
            privileged: Default::default(),
            wasi_capabilities: None,
            fs_policy: None,
            secret_bindings: Default::default(),
            platform: Default::default(),
            idle_timeout_seconds: None,
            no_ttl: Default::default(),
            idle_policy: None,
            vcpu_millis: None,
            disk_mib: None,
            retention: Default::default(),
        };
        edit(&mut parts);
        Ok(LeaseRequest {
            operation_id: None,
            tenant: TenantId::new("t")?,
            provider_pref: vec![ProviderKind::Subprocess],
            spec: SandboxSpec::new(parts)?,
            ttl_seconds: 300,
            idempotency_key: LeaseRequestKey::new("k")?,
            isolation_tier: None,
            host_imports: vec![],
            required_capabilities: None,
            prewarm: Default::default(),
            tags: SandboxTags::default(),
            name: None,
            persistent: Default::default(),
            native: None,
            region: None,
            fallback_regions: Vec::new(),
        })
    }

    async fn lease(&self, id: &str, req: &LeaseRequest) -> Result<u32, TestError> {
        let provided = self.worker.lease(req, &LeaseId::new(id)).await?;
        Ok(provided.sandbox_id.as_str().parse()?)
    }

    async fn until_idle(&self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while self.worker.active_leases() > 0 {
            assert!(Instant::now() < deadline, "the lease did not end");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

fn system32(name: &str) -> Result<PathBuf, TestError> {
    let root = SystemEnv.var("SystemRoot")?;
    Ok(Path::new(&root).join("System32").join(name))
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default().trim().to_string()
}

fn has_exited(pid: u32) -> bool {
    unsafe {
        match OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            Err(_) => true,
            Ok(handle) => {
                let done = WaitForSingleObject(handle, 0) == WAIT_OBJECT_0;
                let _ = CloseHandle(handle);
                done
            }
        }
    }
}

async fn until_exited(pid: u32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if has_exited(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[tokio::test]
async fn a_lease_runs_in_the_appcontainer_with_the_lease_environment() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new("echo %SANDBOX_LEASE_ID%:%SANDBOX_TENANT%> \"%HOME%\\proof.txt\"")?;
    let req = harness.request(|_| {})?;
    harness.lease("lease-1", &req).await?;
    harness.until_idle().await;
    assert_eq!(read(&harness.workspace.join("proof.txt")), "lease-1:t");
    Ok(())
}

#[tokio::test]
async fn the_lease_cannot_open_a_tcp_connection() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let curl = system32("curl.exe")?;
    let harness = Harness::new(&format!(
        "echo control> \"%HOME%\\control.txt\" & \"{}\" -s -m 3 -o NUL http://127.0.0.1:{port}/ & >\"%HOME%\\code.txt\" call echo %^errorlevel%",
        curl.display()
    ))?;
    let req = harness.request(|spec| spec.network_policy = Some(NetworkPolicy::DenyAll))?;
    harness.lease("lease-net", &req).await?;
    harness.until_idle().await;
    assert_eq!(read(&harness.workspace.join("control.txt")), "control");
    let code = read(&harness.workspace.join("code.txt"));
    assert!(code == "7" || code == "28", "curl exited {code}");
    assert!(listener.accept().is_err(), "the listener on the host saw a connection");
    Ok(())
}

#[tokio::test]
async fn a_lease_that_needs_more_than_the_worker_advertises_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new("exit /b 0")?;
    let user_defined = NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    type SpecChange = Box<dyn FnOnce(&mut SandboxSpecParts)>;
    let cases: Vec<(&str, SpecChange)> = vec![
        (
            "allow all",
            Box::new(|spec| spec.network_policy = Some(NetworkPolicy::AllowAll)),
        ),
        (
            "user defined",
            Box::new(move |spec| spec.network_policy = Some(user_defined)),
        ),
        ("another user", Box::new(|spec| spec.user = Some("someone".to_string()))),
        (
            "privileged",
            Box::new(|spec| spec.privileged = PrivilegedAccess::Granted),
        ),
    ];
    for (name, edit) in cases {
        let req = harness.request(edit)?;
        assert!(harness.lease(name, &req).await.is_err(), "{name}");
        assert_eq!(harness.worker.active_leases(), 0, "{name}");
    }
    Ok(())
}

#[tokio::test]
async fn a_workspace_outside_the_root_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new("exit /b 0")?;
    let elsewhere = tempfile::tempdir()?;
    let path = elsewhere.path().to_string_lossy().into_owned();
    let req = harness.request(|spec| {
        spec.workspace = WorkspaceSource::HostMount {
            path,
            access: HostMountAccess::ReadWrite,
        }
    })?;
    assert!(harness.lease("outside", &req).await.is_err());
    assert_eq!(harness.worker.active_leases(), 0);
    Ok(())
}

#[tokio::test]
async fn cancel_ends_the_lease() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new("for /l %i in (1,0,1) do @rem")?;
    let req = harness.request(|_| {})?;
    let pid = harness.lease("lease-cancel", &req).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!has_exited(pid));
    assert_eq!(harness.worker.active_leases(), 1);

    harness.worker.cancel(&LeaseId::new("lease-cancel"));
    assert!(until_exited(pid).await, "the lease survived cancel");
    harness.until_idle().await;
    Ok(())
}

#[tokio::test]
async fn a_lease_that_ends_forgets_its_profile() -> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::new("exit /b 0")?;
    let req = harness.request(|_| {})?;
    harness.lease("lease-done", &req).await?;
    harness.until_idle().await;
    let journal = harness.root.path().join(".appcontainer-profiles");
    assert_eq!(fs::read_dir(&journal)?.count(), 0);
    assert_eq!(harness.worker.sweep_leftover_profiles(), 0);
    Ok(())
}
