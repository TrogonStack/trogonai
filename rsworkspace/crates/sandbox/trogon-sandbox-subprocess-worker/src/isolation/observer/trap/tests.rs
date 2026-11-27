use std::collections::HashMap;

use super::*;

#[derive(Default)]
struct Fake {
    strings: HashMap<u64, Vec<u8>>,
    words: HashMap<u64, u64>,
    cwd: Option<PathBuf>,
    fds: HashMap<i32, PathBuf>,
}

impl Tracee for Fake {
    fn read_c_string(&self, address: u64) -> Option<Vec<u8>> {
        self.strings.get(&address).cloned()
    }
    fn read_u64(&self, address: u64) -> Option<u64> {
        self.words.get(&address).copied()
    }
    fn cwd(&self) -> Option<PathBuf> {
        self.cwd.clone()
    }
    fn fd_path(&self, fd: i32) -> Option<PathBuf> {
        self.fds.get(&fd).cloned()
    }
}

fn tracee(path: &str) -> Fake {
    let mut fake = Fake::default();
    fake.strings.insert(0x1000, path.as_bytes().to_vec());
    fake.cwd = Some(PathBuf::from("/workspace/sub"));
    fake.fds.insert(7, PathBuf::from("/data"));
    fake
}

fn at_cwd(extra: u64) -> [u64; 6] {
    [AT_FDCWD as u32 as u64, 0x1000, extra, 0, 0, 0]
}

#[test]
fn a_write_open_is_a_write_and_a_creating_open_is_a_create() -> Result<(), Box<dyn std::error::Error>> {
    let fake = tracee("/etc/hosts");
    let write = describe(libc::SYS_openat, &at_cwd(libc::O_WRONLY as u64), &fake);
    assert_eq!(
        write,
        [Observed {
            operation: FsOperation::Write,
            path: "/etc/hosts".into()
        }]
    );
    let create = describe(
        libc::SYS_openat,
        &at_cwd((libc::O_WRONLY | libc::O_CREAT) as u64),
        &fake,
    );
    assert_eq!(create[0].operation, FsOperation::Create);
    Ok(())
}

#[test]
fn a_read_only_open_is_not_a_write_attempt() -> Result<(), Box<dyn std::error::Error>> {
    let fake = tracee("/etc/hosts");
    assert!(describe(libc::SYS_openat, &at_cwd(libc::O_RDONLY as u64), &fake).is_empty());
    Ok(())
}

#[test]
fn openat2_reads_its_flags_from_the_how_struct() -> Result<(), Box<dyn std::error::Error>> {
    let mut fake = tracee("/etc/hosts");
    fake.words.insert(0x2000, libc::O_RDWR as u64);
    let seen = describe(SYS_OPENAT2, &[AT_FDCWD as u32 as u64, 0x1000, 0x2000, 24, 0, 0], &fake);
    assert_eq!(seen[0].operation, FsOperation::Write);
    fake.words.insert(0x2000, libc::O_RDONLY as u64);
    assert!(describe(SYS_OPENAT2, &[AT_FDCWD as u32 as u64, 0x1000, 0x2000, 24, 0, 0], &fake).is_empty());
    Ok(())
}

#[test]
fn a_relative_path_is_resolved_against_the_cwd_or_the_directory_fd() -> Result<(), Box<dyn std::error::Error>> {
    let fake = tracee("new.txt");
    let seen = describe(libc::SYS_mkdirat, &[AT_FDCWD as u32 as u64, 0x1000, 0, 0, 0, 0], &fake);
    assert_eq!(seen[0].path, "/workspace/sub/new.txt");
    let seen = describe(libc::SYS_mkdirat, &[7, 0x1000, 0, 0, 0, 0], &fake);
    assert_eq!(seen[0].path, "/data/new.txt");
    assert_eq!(seen[0].operation, FsOperation::Create);
    Ok(())
}

#[test]
fn a_rename_reports_both_ends_and_a_link_only_the_new_name() -> Result<(), Box<dyn std::error::Error>> {
    let mut fake = tracee("/a");
    fake.strings.insert(0x2000, b"/b".to_vec());
    let abs = AT_FDCWD as u32 as u64;
    let renamed = describe(libc::SYS_renameat2, &[abs, 0x1000, abs, 0x2000, 0, 0], &fake);
    assert_eq!(
        renamed.iter().map(|o| o.path.as_str()).collect::<Vec<_>>(),
        ["/a", "/b"]
    );
    assert!(renamed.iter().all(|o| o.operation == FsOperation::Rename));
    let linked = describe(libc::SYS_linkat, &[abs, 0x1000, abs, 0x2000, 0, 0], &fake);
    assert_eq!(linked.len(), 1);
    assert_eq!(linked[0].path, "/b");
    assert_eq!(linked[0].operation, FsOperation::Link);
    Ok(())
}

#[test]
fn unreadable_empty_and_non_utf8_paths_are_skipped() -> Result<(), Box<dyn std::error::Error>> {
    let mut fake = Fake::default();
    let args = [AT_FDCWD as u32 as u64, 0x1000, 0, 0, 0, 0];
    assert!(describe(libc::SYS_unlinkat, &args, &fake).is_empty());
    fake.strings.insert(0x1000, Vec::new());
    assert!(describe(libc::SYS_unlinkat, &args, &fake).is_empty());
    fake.strings.insert(0x1000, vec![0xff, 0xfe]);
    assert!(describe(libc::SYS_unlinkat, &args, &fake).is_empty());
    assert!(describe(libc::SYS_utimensat, &[AT_FDCWD as u32 as u64, 0, 0, 0, 0, 0], &fake).is_empty());
    Ok(())
}

#[test]
fn the_program_traps_each_listed_call_and_gates_opens_on_their_flags() -> Result<(), Box<dyn std::error::Error>> {
    let program = observe_program();
    let notify = program
        .iter()
        .position(|i| i.k == RET_USER_NOTIF)
        .ok_or("a notify verdict")?;
    assert_eq!(program.len(), notify + 2);
    let jump_eq = (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16;
    let dispatch: Vec<(usize, &sock_filter)> = program
        .iter()
        .enumerate()
        .filter(|(index, i)| *index >= 4 && i.code == jump_eq)
        .collect();
    assert_eq!(dispatch.len(), traps().count());
    for (index, instruction) in dispatch {
        let trap = traps()
            .find(|t| t.nr as u32 == instruction.k)
            .ok_or("trap for the jump")?;
        let target = index + 1 + instruction.jt as usize;
        match trap.gate {
            Gate::Always => assert_eq!(target, notify, "nr {}", trap.nr),
            Gate::Flags(arg) => {
                assert_eq!(program[target].k, OFFSET_ARGS + 8 * arg as u32);
                let test = &program[target + 1];
                assert_eq!(test.k, WRITE_INTENT);
                assert_eq!(target + 2 + test.jt as usize, notify);
                assert_eq!(program[target + 2 + test.jf as usize].k, libc::SECCOMP_RET_ALLOW);
            }
        }
    }
    Ok(())
}

#[test]
fn a_foreign_architecture_is_allowed_through() -> Result<(), Box<dyn std::error::Error>> {
    let program = observe_program();
    assert_eq!(program[0].k, OFFSET_ARCH);
    assert_eq!(program[1].k, AUDIT_ARCH);
    assert_eq!(program[2].k, libc::SECCOMP_RET_ALLOW);
    Ok(())
}
