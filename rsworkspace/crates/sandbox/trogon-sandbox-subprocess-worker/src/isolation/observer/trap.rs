//! Which syscalls the observer traps and how a trapped call becomes a path
//! and an operation. Nothing here changes what the kernel does with a call.

use std::path::PathBuf;

use nix::libc;
use seccompiler::{BpfProgram, sock_filter};
use trogon_sandbox_contract::FsOperation;

pub(super) use super::constants::{AT_FDCWD, PATH_MAX, RET_USER_NOTIF};
use super::constants::{AUDIT_ARCH, OFFSET_ARCH, OFFSET_ARGS, OFFSET_NR, SYS_FCHMODAT2, SYS_OPENAT2, WRITE_INTENT};

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!("the filesystem observer supports only x86_64 and aarch64");
#[cfg(not(target_endian = "little"))]
compile_error!("the filesystem observer reads syscall arguments little-endian");

#[derive(Clone, Copy, Debug)]
enum Target {
    At { dirfd: usize, path: usize },
    Plain { path: usize },
}

#[derive(Clone, Copy, Debug)]
enum Operation {
    Fixed(FsOperation),
    OpenFlags(usize),
    OpenHow(usize),
}

#[derive(Clone, Copy, Debug)]
enum Gate {
    Always,
    Flags(usize),
}

#[derive(Clone, Copy, Debug)]
struct Trap {
    nr: i64,
    gate: Gate,
    targets: &'static [Target],
    operation: Operation,
}

const fn at(dirfd: usize, path: usize) -> Target {
    Target::At { dirfd, path }
}

const fn plain(path: usize) -> Target {
    Target::Plain { path }
}

const fn always(nr: i64, targets: &'static [Target], operation: FsOperation) -> Trap {
    Trap {
        nr,
        gate: Gate::Always,
        targets,
        operation: Operation::Fixed(operation),
    }
}

fn traps() -> impl Iterator<Item = &'static Trap> {
    common_traps().iter().chain(legacy_traps().iter())
}

fn instruction(code: u32, jt: u8, jf: u8, k: u32) -> sock_filter {
    sock_filter {
        code: code as u16,
        jt,
        jf,
        k,
    }
}

/// The filter that sends write-intent filesystem calls to the supervisor and
/// lets everything else through untouched.
pub(super) fn observe_program() -> BpfProgram {
    let load = libc::BPF_LD | libc::BPF_W | libc::BPF_ABS;
    let jump_eq = libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K;
    let jump_set = libc::BPF_JMP | libc::BPF_JSET | libc::BPF_K;
    let ret = libc::BPF_RET | libc::BPF_K;

    let all: Vec<&Trap> = traps().collect();
    let gated: Vec<&Trap> = all
        .iter()
        .copied()
        .filter(|trap| matches!(trap.gate, Gate::Flags(_)))
        .collect();

    let dispatch_start = 4;
    let fallthrough = dispatch_start + all.len();
    let handlers_start = fallthrough + 1;
    let notify = handlers_start + gated.len() * 2;
    let allow = notify + 1;

    // A jump too far for the instruction saturates, and the kernel then
    // refuses the program instead of running it with a wrong target.
    let relative = |from: usize, to: usize| u8::try_from(to - from - 1).unwrap_or(u8::MAX);

    let mut program = vec![
        instruction(load, 0, 0, OFFSET_ARCH),
        instruction(jump_eq, 1, 0, AUDIT_ARCH),
        instruction(ret, 0, 0, libc::SECCOMP_RET_ALLOW),
        instruction(load, 0, 0, OFFSET_NR),
    ];
    let mut next_handler = handlers_start;
    for (index, trap) in all.iter().enumerate() {
        let here = dispatch_start + index;
        let target = match trap.gate {
            Gate::Always => notify,
            Gate::Flags(_) => {
                let handler = next_handler;
                next_handler += 2;
                handler
            }
        };
        program.push(instruction(jump_eq, relative(here, target), 0, trap.nr as u32));
    }
    program.push(instruction(ret, 0, 0, libc::SECCOMP_RET_ALLOW));
    for (index, trap) in gated.iter().enumerate() {
        let Gate::Flags(arg) = trap.gate else {
            continue;
        };
        let load_at = handlers_start + index * 2;
        program.push(instruction(load, 0, 0, OFFSET_ARGS + 8 * arg as u32));
        program.push(instruction(
            jump_set,
            relative(load_at + 1, notify),
            relative(load_at + 1, allow),
            WRITE_INTENT,
        ));
    }
    program.push(instruction(ret, 0, 0, RET_USER_NOTIF));
    program.push(instruction(ret, 0, 0, libc::SECCOMP_RET_ALLOW));
    program
}

/// Reads from the process that made the call, whose memory is the only place
/// the path lives.
pub(super) trait Tracee {
    fn read_c_string(&self, address: u64) -> Option<Vec<u8>>;
    fn read_u64(&self, address: u64) -> Option<u64>;
    fn cwd(&self) -> Option<PathBuf>;
    fn fd_path(&self, fd: i32) -> Option<PathBuf>;
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) struct Observed {
    pub operation: FsOperation,
    pub path: String,
}

/// What a trapped call asks to do, with each path made absolute but not
/// resolved: symlinks and mounts are the kernel's business.
pub(super) fn describe(nr: i64, args: &[u64; 6], tracee: &dyn Tracee) -> Vec<Observed> {
    let Some(trap) = traps().find(|trap| trap.nr == nr) else {
        return Vec::new();
    };
    let Some(operation) = operation_of(trap, args, tracee) else {
        return Vec::new();
    };
    trap.targets
        .iter()
        .filter_map(|target| absolute_path(*target, args, tracee))
        .map(|path| Observed { operation, path })
        .collect()
}

fn operation_of(trap: &Trap, args: &[u64; 6], tracee: &dyn Tracee) -> Option<FsOperation> {
    match trap.operation {
        Operation::Fixed(operation) => Some(operation),
        Operation::OpenFlags(index) => open_operation(args[index] as u32 as u64),
        Operation::OpenHow(index) => open_operation(tracee.read_u64(args[index])?),
    }
}

fn open_operation(flags: u64) -> Option<FsOperation> {
    let flags = flags as u32;
    if flags & WRITE_INTENT == 0 {
        None
    } else if flags & libc::O_CREAT as u32 != 0 {
        Some(FsOperation::Create)
    } else {
        Some(FsOperation::Write)
    }
}

fn absolute_path(target: Target, args: &[u64; 6], tracee: &dyn Tracee) -> Option<String> {
    let (dirfd, path) = match target {
        Target::At { dirfd, path } => (Some(args[dirfd] as u32 as i32), args[path]),
        Target::Plain { path } => (None, args[path]),
    };
    if path == 0 {
        return None;
    }
    let raw = tracee.read_c_string(path)?;
    let raw = String::from_utf8(raw).ok()?;
    if raw.is_empty() {
        return None;
    }
    if raw.starts_with('/') {
        return Some(raw);
    }
    let base = match dirfd {
        Some(fd) if fd != AT_FDCWD => tracee.fd_path(fd)?,
        _ => tracee.cwd()?,
    };
    let base = base.to_str()?.trim_end_matches('/');
    Some(format!("{base}/{raw}"))
}

fn common_traps() -> &'static [Trap] {
    const TABLE: &[Trap] = &[
        Trap {
            nr: libc::SYS_openat,
            gate: Gate::Flags(2),
            targets: &[at(0, 1)],
            operation: Operation::OpenFlags(2),
        },
        Trap {
            nr: SYS_OPENAT2,
            gate: Gate::Always,
            targets: &[at(0, 1)],
            operation: Operation::OpenHow(2),
        },
        always(libc::SYS_unlinkat, &[at(0, 1)], FsOperation::Delete),
        always(libc::SYS_mkdirat, &[at(0, 1)], FsOperation::Create),
        always(libc::SYS_mknodat, &[at(0, 1)], FsOperation::Create),
        always(libc::SYS_symlinkat, &[at(1, 2)], FsOperation::Link),
        always(libc::SYS_linkat, &[at(2, 3)], FsOperation::Link),
        always(libc::SYS_renameat2, &[at(0, 1), at(2, 3)], FsOperation::Rename),
        always(libc::SYS_fchmodat, &[at(0, 1)], FsOperation::SetMetadata),
        always(SYS_FCHMODAT2, &[at(0, 1)], FsOperation::SetMetadata),
        always(libc::SYS_fchownat, &[at(0, 1)], FsOperation::SetMetadata),
        always(libc::SYS_utimensat, &[at(0, 1)], FsOperation::SetMetadata),
        always(libc::SYS_truncate, &[plain(0)], FsOperation::Write),
    ];
    TABLE
}

#[cfg(target_arch = "x86_64")]
fn legacy_traps() -> &'static [Trap] {
    const TABLE: &[Trap] = &[
        Trap {
            nr: libc::SYS_open,
            gate: Gate::Flags(1),
            targets: &[plain(0)],
            operation: Operation::OpenFlags(1),
        },
        always(libc::SYS_creat, &[plain(0)], FsOperation::Create),
        always(libc::SYS_unlink, &[plain(0)], FsOperation::Delete),
        always(libc::SYS_rmdir, &[plain(0)], FsOperation::Delete),
        always(libc::SYS_mkdir, &[plain(0)], FsOperation::Create),
        always(libc::SYS_mknod, &[plain(0)], FsOperation::Create),
        always(libc::SYS_symlink, &[plain(1)], FsOperation::Link),
        always(libc::SYS_link, &[plain(1)], FsOperation::Link),
        always(libc::SYS_rename, &[plain(0), plain(1)], FsOperation::Rename),
        always(libc::SYS_renameat, &[at(0, 1), at(2, 3)], FsOperation::Rename),
        always(libc::SYS_chmod, &[plain(0)], FsOperation::SetMetadata),
        always(libc::SYS_chown, &[plain(0)], FsOperation::SetMetadata),
        always(libc::SYS_lchown, &[plain(0)], FsOperation::SetMetadata),
        always(libc::SYS_utime, &[plain(0)], FsOperation::SetMetadata),
        always(libc::SYS_utimes, &[plain(0)], FsOperation::SetMetadata),
        always(libc::SYS_futimesat, &[at(0, 1)], FsOperation::SetMetadata),
    ];
    TABLE
}

#[cfg(not(target_arch = "x86_64"))]
fn legacy_traps() -> &'static [Trap] {
    &[]
}

#[cfg(test)]
mod tests;
