use std::convert::Infallible;
use std::ffi::CString;
use std::io;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::process::{Command, Stdio};

use trogon_sandbox_guest_init::{GuestAgentPort, GuestBootConfig};

use crate::guest_init_error::GuestInitError;

/// Everything the guest init asks of the operating system, so the boot sequence can be exercised
/// without being PID 1.
trait Host {
    fn process_id(&self) -> u32;
    fn mount(
        &mut self,
        source: &str,
        target: &str,
        filesystem: &str,
        flags: libc::c_ulong,
        data: &str,
    ) -> io::Result<()>;
    fn create_dir_all(&mut self, path: &str) -> io::Result<()>;
    fn device(&mut self, path: &str, major: u32, minor: u32) -> io::Result<()>;
    fn remove_file(&mut self, path: &str) -> io::Result<()>;
    fn symlink(&mut self, target: &str, link: &str) -> io::Result<()>;
    fn read_to_string(&mut self, path: &str) -> io::Result<String>;
    fn write(&mut self, path: &str, contents: &str) -> io::Result<()>;
    fn spawn_agent(&mut self, port: GuestAgentPort) -> io::Result<u32>;
    /// Waits for any child and returns its process id and wait status.
    fn wait(&mut self) -> io::Result<(u32, i32)>;
}

struct System;

impl Host for System {
    fn process_id(&self) -> u32 {
        std::process::id()
    }

    fn mount(
        &mut self,
        source: &str,
        target: &str,
        filesystem: &str,
        flags: libc::c_ulong,
        data: &str,
    ) -> io::Result<()> {
        std::fs::create_dir_all(target)?;
        tolerate_busy(raw_mount(source, target, filesystem, flags, data))
    }

    fn create_dir_all(&mut self, path: &str) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn device(&mut self, path: &str, major: u32, minor: u32) -> io::Result<()> {
        device_with(path, |path| make_node(path, major, minor))
    }

    fn remove_file(&mut self, path: &str) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    fn symlink(&mut self, target: &str, link: &str) -> io::Result<()> {
        symlink(target, link)
    }

    fn read_to_string(&mut self, path: &str) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn write(&mut self, path: &str, contents: &str) -> io::Result<()> {
        std::fs::write(path, contents)
    }

    fn spawn_agent(&mut self, port: GuestAgentPort) -> io::Result<u32> {
        agent_command(port).spawn().map(|agent| agent.id())
    }

    fn wait(&mut self) -> io::Result<(u32, i32)> {
        let mut status = 0;
        let pid = unsafe { libc::waitpid(-1, &mut status, 0) };
        (pid >= 0)
            .then_some((pid as u32, status))
            .ok_or_else(io::Error::last_os_error)
    }
}

fn raw_mount(source: &str, target: &str, filesystem: &str, flags: libc::c_ulong, data: &str) -> io::Result<()> {
    let source = CString::new(source)?;
    let target = CString::new(target)?;
    let filesystem = CString::new(filesystem)?;
    let data = CString::new(data)?;
    let result = unsafe {
        libc::mount(
            source.as_ptr(),
            target.as_ptr(),
            filesystem.as_ptr(),
            flags,
            data.as_ptr().cast(),
        )
    };
    (result == 0).then_some(()).ok_or_else(io::Error::last_os_error)
}

/// A filesystem that is already mounted is as good as one this init just mounted.
fn tolerate_busy(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(error) if error.raw_os_error() != Some(libc::EBUSY) => Err(error),
        _ => Ok(()),
    }
}

fn make_node(path: &str, major: u32, minor: u32) -> io::Result<()> {
    let target = CString::new(path)?;
    let result = unsafe { libc::mknod(target.as_ptr(), libc::S_IFCHR | 0o666, libc::makedev(major, minor)) };
    (result == 0).then_some(()).ok_or_else(io::Error::last_os_error)
}

/// Creates a device node and opens its permissions to everyone, whatever the umask said.
fn device_with(path: &str, make: impl FnOnce(&str) -> io::Result<()>) -> io::Result<()> {
    make(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666))
}

fn agent_command(port: GuestAgentPort) -> Command {
    let mut command = Command::new("/.sandbox/agent");
    command
        .args(["--vsock-port", &port.to_string()])
        .env_clear()
        .env("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
        .env("HOME", "/root")
        .env("LANG", "C.UTF-8")
        .stdin(Stdio::null());
    command
}

pub fn run() -> Result<Infallible, GuestInitError> {
    run_with(&mut System)
}

/// Keeps a failed PID 1 alive, because a guest whose init exits panics the kernel. Any other
/// process just returns so the caller can exit.
pub fn linger_as_init(process_id: u32, mut park: impl FnMut() -> bool) {
    while process_id == 1 && park() {}
}

pub fn park_once() -> bool {
    std::thread::park();
    true
}

fn run_with(host: &mut impl Host) -> Result<Infallible, GuestInitError> {
    if host.process_id() != 1 {
        return Err(GuestInitError::NotGuestPidOne);
    }
    let restricted = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;
    host.mount("proc", "/proc", "proc", restricted, "")?;
    host.mount("sysfs", "/sys", "sysfs", restricted, "")?;
    host.mount("tmpfs", "/dev", "tmpfs", libc::MS_NOSUID, "mode=0755")?;
    for (path, major, minor) in [
        ("/dev/null", 1, 3),
        ("/dev/zero", 1, 5),
        ("/dev/full", 1, 7),
        ("/dev/random", 1, 8),
        ("/dev/urandom", 1, 9),
        ("/dev/tty", 5, 0),
        ("/dev/fuse", 10, 229),
    ] {
        host.device(path, major, minor)?;
    }
    host.create_dir_all("/dev/net")?;
    host.device("/dev/net/tun", 10, 200)?;
    host.mount("tmpfs", "/run", "tmpfs", libc::MS_NOSUID | libc::MS_NODEV, "mode=0755")?;
    host.mount("tmpfs", "/tmp", "tmpfs", libc::MS_NOSUID | libc::MS_NODEV, "mode=1777")?;
    host.mount(
        "devpts",
        "/dev/pts",
        "devpts",
        libc::MS_NOSUID | libc::MS_NOEXEC,
        "newinstance,ptmxmode=0666,mode=0620",
    )?;
    match host.remove_file("/dev/ptmx") {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    host.symlink("pts/ptmx", "/dev/ptmx")?;
    host.mount(
        "tmpfs",
        "/dev/shm",
        "tmpfs",
        libc::MS_NOSUID | libc::MS_NODEV,
        "mode=1777",
    )?;
    host.mount("cgroup2", "/sys/fs/cgroup", "cgroup2", restricted, "")?;
    for (target, link) in [
        ("/proc/self/fd", "/dev/fd"),
        ("/proc/self/fd/0", "/dev/stdin"),
        ("/proc/self/fd/1", "/dev/stdout"),
        ("/proc/self/fd/2", "/dev/stderr"),
    ] {
        match host.symlink(target, link) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    let config = GuestBootConfig::from_kernel_command_line(&host.read_to_string("/proc/cmdline")?)?;
    host.create_dir_all("/run/ate")?;
    host.write("/run/ate/actor-id", config.actor().as_str())?;
    let agent = host.spawn_agent(config.agent_port())?;
    loop {
        match host.wait() {
            Ok((pid, wait_status)) if pid == agent => return Err(GuestInitError::AgentExited { wait_status }),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests;
