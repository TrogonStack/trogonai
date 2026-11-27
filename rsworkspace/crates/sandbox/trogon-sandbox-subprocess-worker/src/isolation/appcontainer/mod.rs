//! Native Windows isolation: one AppContainer and one job object per lease.
//!
//! The payload runs under a token with no capabilities, so it has no network
//! and can read and write only what grants its AppContainer SID. The worker
//! grants that SID access to two directories it creates itself, the workspace
//! of the lease and a scratch directory, and to nothing the user owns. Nothing
//! machine-global is touched: there is no install step, no firewall rule, no
//! local account and no loopback exemption.

mod acl;
mod container;
mod handle;
mod job;

use std::ffi::c_void;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, bail, ensure};
use tempfile::TempDir;
use trogon_std::env::{ReadEnv as _, SystemEnv};
use windows::Win32::Foundation::{GetHandleInformation, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Security::SECURITY_CAPABILITIES;
use windows::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
    PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};
use windows::core::{PCWSTR, PWSTR};

use self::acl::Access;
use self::container::Container;
use self::handle::OwnedHandle;
use self::job::Job;
use super::profile_journal::ProfileJournal;
use super::win_command::{command_line, environment_block, sandbox_env};
use crate::launch::{EgressPolicy, LaunchSpec};
mod constants;
use constants::{HANDLE_FLAG_INHERIT, SCRATCH_PREFIX};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Canonical paths come back as `\\?\C:\dir`. Programs that do not expect the
/// prefix, `cmd.exe` first among them, fail on it, so a plain drive path is
/// given back without it.
fn plain(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path,
    }
}

/// Launches payloads under per-lease AppContainers. The workspace of every
/// launch must be inside `workspace_root`: the worker only changes access
/// lists of directories it owns.
#[derive(Debug, Clone)]
pub struct AppContainerBackend {
    workspace_root: PathBuf,
    journal: ProfileJournal,
}

impl AppContainerBackend {
    pub fn new(workspace_root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let workspace_root = plain(
            workspace_root
                .as_ref()
                .canonicalize()
                .with_context(|| format!("workspace root {}", workspace_root.as_ref().display()))?,
        );
        let journal = ProfileJournal::in_root(&workspace_root);
        Ok(Self {
            workspace_root,
            journal,
        })
    }

    /// Deletes the AppContainer profiles that a worker with the same workspace
    /// root created and did not get to delete, and returns how many it
    /// deleted. Only profiles this backend recorded are touched. Run it
    /// before any lease starts, while no other worker uses the root.
    pub fn sweep_leftover_profiles(&self) -> usize {
        container::sweep(&self.journal)
    }

    pub fn launch(&self, spec: &LaunchSpec) -> anyhow::Result<AppContainerProcess> {
        refuse_unenforceable(spec)?;
        let workspace = plain(
            spec.workspace_host_path
                .canonicalize()
                .with_context(|| format!("workspace {}", spec.workspace_host_path.display()))?,
        );
        ensure!(
            workspace.starts_with(&self.workspace_root) && workspace != self.workspace_root,
            "workspace {} is not inside the workspace root {}",
            workspace.display(),
            self.workspace_root.display()
        );

        let container = Container::create(&self.journal)?;
        let scratch = tempfile::Builder::new()
            .prefix(SCRATCH_PREFIX)
            .tempdir_in(&self.workspace_root)
            .context("creating the scratch directory")?;
        let scratch_path = plain(scratch.path().to_path_buf());
        let workspace_access = if spec.workspace_readonly {
            Access::Read
        } else {
            Access::Full
        };
        acl::grant(&workspace, container.sid(), workspace_access)?;
        acl::grant(&scratch_path, container.sid(), Access::Full)?;

        let job = Job::create(&spec.limits)?;
        let env = sandbox_env(&spec.env, |name| SystemEnv.var(name).ok(), &workspace, &scratch_path);
        let process = spawn(spec, &container, &job, &env, &workspace)?;
        Ok(AppContainerProcess {
            pid: process.pid,
            process: Arc::new(process.handle),
            job,
            _container: container,
            _scratch: scratch,
        })
    }
}

fn refuse_unenforceable(spec: &LaunchSpec) -> anyhow::Result<()> {
    ensure!(
        spec.binary.is_absolute(),
        "{} is not an absolute path; an AppContainer cannot search the PATH of the worker",
        spec.binary.display()
    );
    ensure!(
        spec.fs_policy.is_none(),
        "the AppContainer backend cannot enforce a filesystem policy"
    );
    ensure!(
        spec.extra_binds.is_empty(),
        "the AppContainer backend cannot bind extra paths into the sandbox"
    );
    ensure!(
        spec.user.is_none(),
        "the AppContainer backend cannot run the payload as another user"
    );
    match spec.egress_policy {
        EgressPolicy::DenyAll => Ok(()),
        EgressPolicy::AllowAll => {
            bail!("the AppContainer backend cannot allow network egress; it grants no network capability")
        }
        EgressPolicy::Loopback(_) => {
            bail!("the AppContainer backend cannot reach a loopback proxy; it grants no network capability")
        }
    }
}

struct Spawned {
    pid: u32,
    handle: OwnedHandle,
}

/// Ends the suspended child if setting it up fails before it is resumed.
struct SuspendedChild {
    process: Option<OwnedHandle>,
    thread: OwnedHandle,
}

impl Drop for SuspendedChild {
    fn drop(&mut self) {
        if let Some(process) = &self.process {
            unsafe {
                let _ = TerminateProcess(process.raw(), 1);
            }
        }
    }
}

struct AttributeList {
    buffer: Vec<u8>,
}

impl AttributeList {
    fn new(count: u32) -> anyhow::Result<Self> {
        let mut size = 0usize;
        // Fails by design with the size needed, which is what is asked for.
        let _ = unsafe { InitializeProcThreadAttributeList(None, count, None, &mut size) };
        let mut buffer = vec![0u8; size];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(buffer.as_mut_ptr().cast());
        unsafe { InitializeProcThreadAttributeList(Some(list), count, None, &mut size) }
            .context("InitializeProcThreadAttributeList")?;
        Ok(Self { buffer })
    }

    fn raw(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        LPPROC_THREAD_ATTRIBUTE_LIST(self.buffer.as_mut_ptr().cast())
    }

    /// The value must stay alive and in place until the process is created.
    unsafe fn set<T>(&mut self, attribute: usize, value: &T, size: usize) -> anyhow::Result<()> {
        unsafe {
            UpdateProcThreadAttribute(
                self.raw(),
                0,
                attribute,
                Some(std::ptr::from_ref(value).cast::<c_void>()),
                size,
                None,
                None,
            )
        }
        .context("UpdateProcThreadAttribute")
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.raw()) };
    }
}

/// The standard handles of the worker that can be handed to the child: only
/// valid ones that are inheritable, because `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`
/// refuses anything else. The child gets no handle where the worker has none
/// to give.
fn inheritable_std_handles() -> [HANDLE; 3] {
    let pick = |id| {
        let Ok(handle) = (unsafe { GetStdHandle(id) }) else {
            return HANDLE::default();
        };
        if handle.is_invalid() || handle.0.is_null() {
            return HANDLE::default();
        }
        let mut flags = 0u32;
        match unsafe { GetHandleInformation(handle, &mut flags) } {
            Ok(()) if flags & HANDLE_FLAG_INHERIT != 0 => handle,
            _ => HANDLE::default(),
        }
    };
    [pick(STD_INPUT_HANDLE), pick(STD_OUTPUT_HANDLE), pick(STD_ERROR_HANDLE)]
}

fn spawn(
    spec: &LaunchSpec,
    container: &Container,
    job: &Job,
    env: &[(String, String)],
    workspace: &Path,
) -> anyhow::Result<Spawned> {
    let application = wide(&spec.binary.to_string_lossy());
    let mut command = wide(&command_line(&spec.binary, &spec.args));
    let directory = wide(&workspace.to_string_lossy());
    let environment = environment_block(env)?;

    let capabilities = SECURITY_CAPABILITIES {
        AppContainerSid: container.sid(),
        Capabilities: std::ptr::null_mut(),
        CapabilityCount: 0,
        Reserved: 0,
    };
    let std_handles = inheritable_std_handles();
    let mut listed: Vec<HANDLE> = Vec::new();
    for handle in std_handles {
        if !handle.0.is_null() && !listed.contains(&handle) {
            listed.push(handle);
        }
    }

    let mut attributes = AttributeList::new(if listed.is_empty() { 1 } else { 2 })?;
    unsafe {
        attributes.set(
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
            &capabilities,
            size_of::<SECURITY_CAPABILITIES>(),
        )?;
        if !listed.is_empty() {
            attributes.set(
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                &listed[0],
                listed.len() * size_of::<HANDLE>(),
            )?;
        }
    }

    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.lpAttributeList = attributes.raw();
    if !listed.is_empty() {
        startup.StartupInfo.dwFlags |= STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = std_handles[0];
        startup.StartupInfo.hStdOutput = std_handles[1];
        startup.StartupInfo.hStdError = std_handles[2];
    }

    let mut information = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessW(
            PCWSTR(application.as_ptr()),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            !listed.is_empty(),
            CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
            Some(environment.as_ptr().cast::<c_void>()),
            PCWSTR(directory.as_ptr()),
            std::ptr::from_ref(&startup.StartupInfo),
            &mut information,
        )
    }
    .with_context(|| format!("starting {} in the AppContainer", spec.binary.display()))?;

    let mut child = SuspendedChild {
        process: Some(OwnedHandle::new(information.hProcess)),
        thread: OwnedHandle::new(information.hThread),
    };
    job.assign(information.hProcess)?;
    ensure!(
        unsafe { ResumeThread(child.thread.raw()) } != u32::MAX,
        "ResumeThread failed"
    );
    let process = child.process.take().context("the process handle is gone")?;
    Ok(Spawned {
        pid: information.dwProcessId,
        handle: process,
    })
}

/// A payload running in its own AppContainer and job object. Dropping this
/// ends the job, which ends every process in it, then deletes the AppContainer
/// profile and the scratch directory.
pub struct AppContainerProcess {
    pid: u32,
    process: Arc<OwnedHandle>,
    job: Job,
    _container: Container,
    _scratch: TempDir,
}

impl AppContainerProcess {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn try_exit_code(&self) -> anyhow::Result<Option<u32>> {
        exit_code_if_done(&self.process)
    }

    pub async fn wait(&self) -> anyhow::Result<u32> {
        let process = Arc::clone(&self.process);
        tokio::task::spawn_blocking(move || {
            let waited = unsafe { WaitForSingleObject(process.raw(), INFINITE) };
            ensure!(waited == WAIT_OBJECT_0, "waiting for the process failed");
            exit_code_if_done(&process)?.context("the process had not exited")
        })
        .await
        .context("waiting for the process")?
    }

    /// Ends the payload and everything it started.
    pub fn kill(&self) {
        self.job.terminate();
    }
}

fn exit_code_if_done(process: &OwnedHandle) -> anyhow::Result<Option<u32>> {
    let waited = unsafe { WaitForSingleObject(process.raw(), 0) };
    if waited == WAIT_TIMEOUT {
        return Ok(None);
    }
    ensure!(waited == WAIT_OBJECT_0, "polling the process failed");
    let mut code = 0u32;
    unsafe { GetExitCodeProcess(process.raw(), &mut code) }.context("GetExitCodeProcess")?;
    Ok(Some(code))
}
