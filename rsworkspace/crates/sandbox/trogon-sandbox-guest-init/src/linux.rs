use std::ffi::CString;
use std::io;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::process::{Command, Stdio};

use trogon_sandbox_guest_init::GuestBootConfig;

fn mount(source: &str, target: &str, filesystem: &str, flags: libc::c_ulong, data: &str) -> io::Result<()> {
    std::fs::create_dir_all(target)?;
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
    if result != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EBUSY) {
            return Err(error);
        }
    }
    Ok(())
}

fn device(path: &str, major: u32, minor: u32) -> io::Result<()> {
    let target = CString::new(path)?;
    if unsafe { libc::mknod(target.as_ptr(), libc::S_IFCHR | 0o666, libc::makedev(major, minor)) } != 0 {
        return Err(io::Error::last_os_error());
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666))
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    if std::process::id() != 1 {
        return Err("trogon-sandbox-guest-init must run as guest PID 1".into());
    }
    let restricted = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;
    mount("proc", "/proc", "proc", restricted, "")?;
    mount("sysfs", "/sys", "sysfs", restricted, "")?;
    mount("tmpfs", "/dev", "tmpfs", libc::MS_NOSUID, "mode=0755")?;
    for (path, major, minor) in [
        ("/dev/null", 1, 3),
        ("/dev/zero", 1, 5),
        ("/dev/full", 1, 7),
        ("/dev/random", 1, 8),
        ("/dev/urandom", 1, 9),
        ("/dev/tty", 5, 0),
        ("/dev/fuse", 10, 229),
    ] {
        device(path, major, minor)?;
    }
    std::fs::create_dir_all("/dev/net")?;
    device("/dev/net/tun", 10, 200)?;
    mount("tmpfs", "/run", "tmpfs", libc::MS_NOSUID | libc::MS_NODEV, "mode=0755")?;
    mount("tmpfs", "/tmp", "tmpfs", libc::MS_NOSUID | libc::MS_NODEV, "mode=1777")?;
    mount(
        "devpts",
        "/dev/pts",
        "devpts",
        libc::MS_NOSUID | libc::MS_NOEXEC,
        "newinstance,ptmxmode=0666,mode=0620",
    )?;
    match std::fs::remove_file("/dev/ptmx") {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    symlink("pts/ptmx", "/dev/ptmx")?;
    mount(
        "tmpfs",
        "/dev/shm",
        "tmpfs",
        libc::MS_NOSUID | libc::MS_NODEV,
        "mode=1777",
    )?;
    mount("cgroup2", "/sys/fs/cgroup", "cgroup2", restricted, "")?;
    for (target, link) in [
        ("/proc/self/fd", "/dev/fd"),
        ("/proc/self/fd/0", "/dev/stdin"),
        ("/proc/self/fd/1", "/dev/stdout"),
        ("/proc/self/fd/2", "/dev/stderr"),
    ] {
        match symlink(target, link) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    let config = GuestBootConfig::from_kernel_command_line(&std::fs::read_to_string("/proc/cmdline")?)?;
    std::fs::create_dir_all("/run/ate")?;
    std::fs::write("/run/ate/actor-id", config.actor_id())?;
    let agent = Command::new("/.sandbox/agent")
        .args(["--vsock-port", &config.agent_port().to_string()])
        .env_clear()
        .env("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
        .env("HOME", "/root")
        .env("LANG", "C.UTF-8")
        .stdin(Stdio::null())
        .spawn()?;
    loop {
        let mut status = 0;
        let pid = unsafe { libc::waitpid(-1, &mut status, 0) };
        if pid < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }
        if pid as u32 == agent.id() {
            return Err(format!("guest agent exited with wait status {status}").into());
        }
    }
}
