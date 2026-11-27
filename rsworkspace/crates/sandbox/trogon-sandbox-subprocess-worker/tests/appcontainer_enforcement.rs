#![cfg(windows)]

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use trogon_sandbox_subprocess_worker::isolation::appcontainer::{AppContainerBackend, AppContainerProcess};
use trogon_sandbox_subprocess_worker::isolation::resource_limits::ResourceLimits;
use trogon_sandbox_subprocess_worker::{EgressPolicy, LaunchSpec};
use trogon_std::env::{ReadEnv as _, SystemEnv};
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};

type TestError = Box<dyn std::error::Error>;

struct Lease {
    root: tempfile::TempDir,
    workspace: PathBuf,
}

impl Lease {
    fn new() -> Result<Self, TestError> {
        let root = tempfile::tempdir()?;
        let workspace = root.path().join("workspace");
        fs::create_dir(&workspace)?;
        Ok(Self { root, workspace })
    }

    fn backend(&self) -> Result<AppContainerBackend, TestError> {
        Ok(AppContainerBackend::new(self.root.path())?)
    }

    fn spec(&self, binary: PathBuf, args: &[&str]) -> LaunchSpec {
        LaunchSpec {
            binary,
            args: args.iter().map(|arg| arg.to_string()).collect(),
            env: vec![("LEASE_VAR".to_string(), "from-lease".to_string())],
            workspace_host_path: self.workspace.clone(),
            workspace_sandbox_path: self.workspace.clone(),
            workspace_readonly: false,
            egress_policy: EgressPolicy::DenyAll,
            extra_binds: vec![],
            user: None,
            limits: ResourceLimits::default(),
            fs_policy: None,
            platform: Default::default(),
            fs_denials: None,
        }
    }

    fn cmd(&self, script: &str) -> Result<LaunchSpec, TestError> {
        Ok(self.spec(system32("cmd.exe")?, &["/d", "/s", "/c", script]))
    }
}

fn system32(name: &str) -> Result<PathBuf, TestError> {
    let root = SystemEnv.var("SystemRoot")?;
    Ok(Path::new(&root).join("System32").join(name))
}

async fn run(backend: &AppContainerBackend, spec: &LaunchSpec) -> Result<u32, TestError> {
    let process = backend.launch(spec)?;
    Ok(tokio::time::timeout(Duration::from_secs(60), process.wait()).await??)
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default().trim().to_string()
}

#[tokio::test]
async fn the_payload_can_write_the_workspace() -> Result<(), Box<dyn std::error::Error>> {
    let lease = Lease::new()?;
    let code = run(&lease.backend()?, &lease.cmd("echo written> \"%HOME%\\proof.txt\"")?).await?;
    assert_eq!(code, 0);
    assert_eq!(read(&lease.workspace.join("proof.txt")), "written");
    Ok(())
}

#[tokio::test]
async fn a_read_only_workspace_refuses_writes() -> Result<(), Box<dyn std::error::Error>> {
    let lease = Lease::new()?;
    let mut spec = lease.cmd("echo written> \"%HOME%\\proof.txt\"")?;
    spec.workspace_readonly = true;
    let code = run(&lease.backend()?, &spec).await?;
    assert_ne!(code, 0);
    assert!(!lease.workspace.join("proof.txt").exists());
    Ok(())
}

#[tokio::test]
async fn the_payload_cannot_read_a_file_in_the_users_profile() -> Result<(), Box<dyn std::error::Error>> {
    let profile = PathBuf::from(SystemEnv.var("USERPROFILE")?);
    let secret = tempfile::Builder::new()
        .prefix("appcontainer-secret-")
        .tempdir_in(&profile)?;
    let secret_file = secret.path().join("secret.txt");
    fs::write(&secret_file, "do-not-leak")?;

    let lease = Lease::new()?;
    let script = format!(
        "echo control> \"%HOME%\\control.txt\" & type \"{}\" > \"%HOME%\\leak.txt\" & exit /b 0",
        secret_file.display()
    );
    let code = run(&lease.backend()?, &lease.cmd(&script)?).await?;
    assert_eq!(code, 0, "the script itself must run");
    assert_eq!(
        read(&lease.workspace.join("control.txt")),
        "control",
        "the positive control must have written"
    );
    assert!(
        !read(&lease.workspace.join("leak.txt")).contains("do-not-leak"),
        "the payload read a file outside the lease directories"
    );
    Ok(())
}

#[tokio::test]
async fn the_payload_cannot_open_a_tcp_connection() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let lease = Lease::new()?;
    let backend = lease.backend()?;
    let curl = system32("curl.exe")?;

    let control = run(&backend, &lease.spec(curl.clone(), &["--version"])).await?;
    assert_eq!(control, 0, "curl must start inside the AppContainer");

    let url = format!("http://127.0.0.1:{port}/");
    let code = run(&backend, &lease.spec(curl, &["-s", "-m", "3", "-o", "NUL", &url])).await?;
    assert!(
        code == 7 || code == 28,
        "curl should fail to connect or time out, exited {code}"
    );
    assert!(listener.accept().is_err(), "the listener on the host saw a connection");
    Ok(())
}

#[tokio::test]
async fn the_environment_is_the_allowlist_plus_the_lease() -> Result<(), Box<dyn std::error::Error>> {
    let host_only = SystemEnv.var("COMPUTERNAME")?;
    let lease = Lease::new()?;
    let code = run(&lease.backend()?, &lease.cmd("set > \"%HOME%\\env.txt\"")?).await?;
    assert_eq!(code, 0);
    let env = read(&lease.workspace.join("env.txt")).to_uppercase();
    assert!(env.contains("LEASE_VAR=FROM-LEASE"));
    assert!(env.contains("SYSTEMROOT="));
    assert!(!env.contains("COMPUTERNAME="));
    assert!(!env.contains(&host_only.to_uppercase()));
    Ok(())
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

fn wait_until_exited(pid: u32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if has_exited(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn spin(lease: &Lease) -> Result<AppContainerProcess, TestError> {
    Ok(lease.backend()?.launch(&lease.cmd("for /l %i in (1,0,1) do @rem")?)?)
}

#[tokio::test]
async fn dropping_the_process_ends_it_with_the_job() -> Result<(), Box<dyn std::error::Error>> {
    let lease = Lease::new()?;
    let process = spin(&lease)?;
    let pid = process.pid();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(process.try_exit_code()?, None);
    drop(process);
    assert!(wait_until_exited(pid), "the process outlived its job");
    Ok(())
}

#[tokio::test]
async fn kill_ends_the_process() -> Result<(), Box<dyn std::error::Error>> {
    let lease = Lease::new()?;
    let process = spin(&lease)?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    process.kill();
    let code = tokio::time::timeout(Duration::from_secs(10), process.wait()).await??;
    assert_eq!(code, 1);
    Ok(())
}

#[test]
fn a_workspace_outside_the_root_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let lease = Lease::new()?;
    let other = tempfile::tempdir()?;
    let mut spec = lease.cmd("exit /b 0")?;
    spec.workspace_host_path = other.path().to_path_buf();
    assert!(lease.backend()?.launch(&spec).is_err());
    Ok(())
}

#[test]
fn what_the_backend_cannot_enforce_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let lease = Lease::new()?;
    let backend = lease.backend()?;

    let mut allow_all = lease.cmd("exit /b 0")?;
    allow_all.egress_policy = EgressPolicy::AllowAll;
    assert!(backend.launch(&allow_all).is_err());

    let mut as_user = lease.cmd("exit /b 0")?;
    as_user.user = Some("someone".to_string());
    assert!(backend.launch(&as_user).is_err());

    let relative = lease.spec(PathBuf::from("cmd.exe"), &["/c", "exit"]);
    assert!(backend.launch(&relative).is_err());
    Ok(())
}

#[test]
fn the_sweep_deletes_the_profile_of_a_crashed_worker_and_nothing_else() -> Result<(), Box<dyn std::error::Error>> {
    let lease = Lease::new()?;
    let backend = lease.backend()?;
    let journal = lease.root.path().join(".appcontainer-profiles");
    let process = backend.launch(&lease.cmd("exit /b 0")?)?;
    std::mem::forget(process);
    fs::write(journal.join("not-ours.txt"), "")?;
    assert_eq!(fs::read_dir(&journal)?.count(), 2);

    assert_eq!(backend.sweep_leftover_profiles(), 1);
    assert!(journal.join("not-ours.txt").exists());
    assert_eq!(fs::read_dir(&journal)?.count(), 1);
    assert_eq!(backend.sweep_leftover_profiles(), 0);
    Ok(())
}
