use std::path::PathBuf;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct FakeHost {
    process_id: u32,
    events: Vec<String>,
    failures: Vec<(String, io::ErrorKind)>,
    cmdline: String,
    waits: Vec<io::Result<(u32, i32)>>,
}

impl FakeHost {
    fn init() -> Self {
        Self {
            process_id: 1,
            cmdline: "console=ttyS0 ate.actor_id=actor-7 sandbox.agent_port=9000".to_owned(),
            waits: vec![Ok((42, 3))],
            ..Self::default()
        }
    }

    fn failing(event: &str, kind: io::ErrorKind) -> Self {
        let mut host = Self::init();
        host.failures.push((event.to_owned(), kind));
        host
    }

    fn record(&mut self, event: String) -> io::Result<()> {
        let failure = self
            .failures
            .iter()
            .find(|(failing, _)| *failing == event)
            .map(|(_, kind)| *kind);
        self.events.push(event);
        failure.map_or(Ok(()), |kind| Err(kind.into()))
    }
}

impl Host for FakeHost {
    fn process_id(&self) -> u32 {
        self.process_id
    }

    fn mount(&mut self, _: &str, target: &str, filesystem: &str, flags: libc::c_ulong, data: &str) -> io::Result<()> {
        self.record(format!("mount {filesystem} {target} {flags:#x} {data}"))
    }

    fn create_dir_all(&mut self, path: &str) -> io::Result<()> {
        self.record(format!("mkdir {path}"))
    }

    fn device(&mut self, path: &str, major: u32, minor: u32) -> io::Result<()> {
        self.record(format!("device {path} {major} {minor}"))
    }

    fn remove_file(&mut self, path: &str) -> io::Result<()> {
        self.record(format!("remove {path}"))
    }

    fn symlink(&mut self, target: &str, link: &str) -> io::Result<()> {
        self.record(format!("symlink {target} {link}"))
    }

    fn read_to_string(&mut self, path: &str) -> io::Result<String> {
        self.record(format!("read {path}"))?;
        Ok(self.cmdline.clone())
    }

    fn write(&mut self, path: &str, contents: &str) -> io::Result<()> {
        self.record(format!("write {path} {contents}"))
    }

    fn spawn_agent(&mut self, port: GuestAgentPort) -> io::Result<u32> {
        self.record(format!("spawn {port}"))?;
        Ok(42)
    }

    fn wait(&mut self) -> io::Result<(u32, i32)> {
        self.events.push("wait".to_owned());
        self.waits.remove(0)
    }
}

fn scratch(name: &str) -> io::Result<PathBuf> {
    let directory = std::env::temp_dir().join(format!("guest-init-{}-{name}", std::process::id()));
    match std::fs::remove_dir_all(&directory) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    std::fs::create_dir_all(&directory)?;
    Ok(directory)
}

fn text(path: &std::path::Path) -> Result<&str, Box<dyn std::error::Error>> {
    path.to_str().ok_or_else(|| "scratch path is not UTF-8".into())
}

#[test]
fn boots_in_order_and_reports_the_agent_exit() -> TestResult {
    let mut host = FakeHost::init();
    host.waits = vec![Err(io::ErrorKind::Interrupted.into()), Ok((7, 0)), Ok((42, 256))];
    let result = run_with(&mut host);
    assert!(matches!(result, Err(GuestInitError::AgentExited { wait_status: 256 })));
    assert_eq!(
        host.events,
        [
            "mount proc /proc 0xe ",
            "mount sysfs /sys 0xe ",
            "mount tmpfs /dev 0x2 mode=0755",
            "device /dev/null 1 3",
            "device /dev/zero 1 5",
            "device /dev/full 1 7",
            "device /dev/random 1 8",
            "device /dev/urandom 1 9",
            "device /dev/tty 5 0",
            "device /dev/fuse 10 229",
            "mkdir /dev/net",
            "device /dev/net/tun 10 200",
            "mount tmpfs /run 0x6 mode=0755",
            "mount tmpfs /tmp 0x6 mode=1777",
            "mount devpts /dev/pts 0xa newinstance,ptmxmode=0666,mode=0620",
            "remove /dev/ptmx",
            "symlink pts/ptmx /dev/ptmx",
            "mount tmpfs /dev/shm 0x6 mode=1777",
            "mount cgroup2 /sys/fs/cgroup 0xe ",
            "symlink /proc/self/fd /dev/fd",
            "symlink /proc/self/fd/0 /dev/stdin",
            "symlink /proc/self/fd/1 /dev/stdout",
            "symlink /proc/self/fd/2 /dev/stderr",
            "read /proc/cmdline",
            "mkdir /run/ate",
            "write /run/ate/actor-id actor-7",
            "spawn 9000",
            "wait",
            "wait",
            "wait",
        ]
    );
    Ok(())
}

#[test]
fn refuses_to_run_as_another_process() {
    let mut host = FakeHost::init();
    host.process_id = 2;
    assert!(matches!(run_with(&mut host), Err(GuestInitError::NotGuestPidOne)));
    assert!(host.events.is_empty());
}

#[test]
fn tolerates_a_missing_pty_multiplexer_and_existing_links() {
    let mut host = FakeHost::init();
    host.failures
        .push(("remove /dev/ptmx".to_owned(), io::ErrorKind::NotFound));
    host.failures
        .push(("symlink /proc/self/fd /dev/fd".to_owned(), io::ErrorKind::AlreadyExists));
    assert!(matches!(
        run_with(&mut host),
        Err(GuestInitError::AgentExited { wait_status: 3 })
    ));
}

#[test]
fn stops_at_the_first_failing_step() {
    for event in [
        "mount proc /proc 0xe ",
        "device /dev/fuse 10 229",
        "mkdir /dev/net",
        "mount devpts /dev/pts 0xa newinstance,ptmxmode=0666,mode=0620",
        "remove /dev/ptmx",
        "symlink pts/ptmx /dev/ptmx",
        "mount tmpfs /dev/shm 0x6 mode=1777",
        "symlink /proc/self/fd/2 /dev/stderr",
        "read /proc/cmdline",
        "mkdir /run/ate",
        "write /run/ate/actor-id actor-7",
        "spawn 9000",
    ] {
        let mut host = FakeHost::failing(event, io::ErrorKind::PermissionDenied);
        assert!(matches!(run_with(&mut host), Err(GuestInitError::Io(_))), "{event}");
        assert_eq!(host.events.last().map(String::as_str), Some(event));
    }
}

#[test]
fn rejects_an_unbootable_command_line() {
    let mut host = FakeHost::init();
    host.cmdline = "sandbox.agent_port=0".to_owned();
    assert!(matches!(run_with(&mut host), Err(GuestInitError::BootConfig(_))));
}

#[test]
fn reports_a_failed_wait() {
    let mut host = FakeHost::init();
    host.waits = vec![Err(io::ErrorKind::PermissionDenied.into())];
    assert!(matches!(run_with(&mut host), Err(GuestInitError::Io(_))));
}

#[test]
fn lingers_only_as_pid_one() {
    let mut parked = 0;
    linger_as_init(2, || {
        parked += 1;
        true
    });
    assert_eq!(parked, 0);
    linger_as_init(1, || {
        parked += 1;
        parked < 3
    });
    assert_eq!(parked, 3);
    std::thread::current().unpark();
    assert!(park_once());
}

#[test]
fn tolerates_only_a_busy_mount() {
    assert!(tolerate_busy(Ok(())).is_ok());
    assert!(tolerate_busy(Err(io::Error::from_raw_os_error(libc::EBUSY))).is_ok());
    assert!(tolerate_busy(Err(io::Error::from_raw_os_error(libc::EPERM))).is_err());
}

#[test]
fn opens_device_permissions_after_creating_the_node() -> TestResult {
    let directory = scratch("device")?;
    let path = directory.join("node");
    device_with(text(&path)?, |path| std::fs::write(path, ""))?;
    assert_eq!(std::fs::metadata(&path)?.permissions().mode() & 0o777, 0o666);
    assert!(device_with(text(&path)?, |_| Err(io::ErrorKind::Other.into())).is_err());
    Ok(())
}

#[test]
fn builds_the_agent_command() {
    let command = agent_command(GuestAgentPort::DEFAULT);
    assert_eq!(command.get_program(), "/.sandbox/agent");
    assert_eq!(command.get_args().collect::<Vec<_>>(), ["--vsock-port", "8110"]);
    let environment = command
        .get_envs()
        .map(|(name, value)| (name.to_str(), value.and_then(|value| value.to_str())))
        .collect::<Vec<_>>();
    assert_eq!(
        environment,
        [
            (Some("HOME"), Some("/root")),
            (Some("LANG"), Some("C.UTF-8")),
            (
                Some("PATH"),
                Some("/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
            ),
        ]
    );
}

#[test]
fn system_filesystem_wrappers_act_on_the_filesystem() -> TestResult {
    let directory = scratch("system")?;
    let root = text(&directory)?;
    let mut system = System;
    assert_eq!(system.process_id(), std::process::id());
    let nested = format!("{root}/a/b");
    system.create_dir_all(&nested)?;
    let file = format!("{nested}/file");
    system.write(&file, "contents")?;
    assert_eq!(system.read_to_string(&file)?, "contents");
    let link = format!("{nested}/link");
    system.symlink(&file, &link)?;
    assert_eq!(system.read_to_string(&link)?, "contents");
    assert_eq!(
        system.symlink(&file, &link).err().map(|error| error.kind()),
        Some(io::ErrorKind::AlreadyExists)
    );
    system.remove_file(&link)?;
    assert_eq!(
        system.remove_file(&link).err().map(|error| error.kind()),
        Some(io::ErrorKind::NotFound)
    );
    Ok(())
}

#[test]
fn system_syscall_wrappers_fail_deterministically() -> TestResult {
    let directory = scratch("syscalls")?;
    let root = text(&directory)?;
    let mut system = System;
    let missing_node = format!("{root}/missing/node");
    assert_eq!(
        system.device(&missing_node, 1, 3).err().map(|error| error.kind()),
        Some(io::ErrorKind::NotFound)
    );
    assert_eq!(
        make_node("bad\0path", 1, 3).err().map(|error| error.kind()),
        Some(io::ErrorKind::InvalidInput)
    );
    let target = format!("{root}/mountpoint");
    assert!(system.mount("none", &target, "no-such-filesystem", 0, "").is_err());
    assert!(directory.join("mountpoint").is_dir());
    assert_eq!(
        system
            .mount("none", &target, "bad\0type", 0, "")
            .err()
            .map(|error| error.kind()),
        Some(io::ErrorKind::InvalidInput)
    );
    assert!(system.spawn_agent(GuestAgentPort::DEFAULT).is_err());
    assert_eq!(
        system.wait().err().and_then(|error| error.raw_os_error()),
        Some(libc::ECHILD)
    );
    Ok(())
}
