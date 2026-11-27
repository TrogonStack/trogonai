use std::collections::BTreeMap;

use nix::libc;
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter, SeccompRule,
    TargetArch, sock_filter,
};
use trogon_sandbox_contract::LinuxOptions;

use super::constants::{
    AUDIT_ARCH_X86_64, DENIED_CLONE_FLAGS, DENIED_SOCKET_DOMAINS, DENIED_TTY_IOCTLS, DENIED_UNSHARE_FLAGS,
    IO_URING_SYSCALLS, NAMESPACE_SYSCALLS, SECCOMP_DATA_ARCH_OFFSET, SECCOMP_DATA_NR_OFFSET, TARGET_ARCH,
    X32_SYSCALL_BIT,
};

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!("the bubblewrap seccomp filter supports only x86_64 and aarch64");

/// What the filters let through that they refuse by default.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, clap::Args)]
pub struct SeccompOptions {
    /// Creating `AF_UNIX` and `AF_VSOCK` sockets.
    #[arg(long)]
    pub allow_unix_sockets: bool,
    /// Creating user and mount namespaces, changing namespaces and mounting.
    #[arg(long)]
    pub allow_nested_namespaces: bool,
}

impl SeccompOptions {
    pub fn of(options: &LinuxOptions) -> Self {
        Self {
            allow_unix_sockets: options.allow_all_unix_sockets.is_allowed(),
            allow_nested_namespaces: options.allow_nested_user_namespaces.is_allowed(),
        }
    }

    pub fn flags(&self) -> Vec<String> {
        let mut flags = Vec::new();
        if self.allow_unix_sockets {
            flags.push("--allow-unix-sockets".to_string());
        }
        if self.allow_nested_namespaces {
            flags.push("--allow-nested-namespaces".to_string());
        }
        flags
    }
}

/// Filters to stack inside the sandbox, in install order. Each one is a
/// serialized BPF program in the format bubblewrap's `--add-seccomp-fd`
/// expects.
pub fn build_sandbox_filters(options: SeccompOptions) -> anyhow::Result<Vec<Vec<u8>>> {
    Ok(build_sandbox_programs(options)?.iter().map(serialize_program).collect())
}

/// The same filters as [`build_sandbox_filters`], for a process that installs
/// them on itself instead of through bubblewrap.
pub fn build_sandbox_programs(options: SeccompOptions) -> anyhow::Result<Vec<BpfProgram>> {
    let mut filters = vec![build_deny_filter(options)?];
    if !options.allow_nested_namespaces {
        filters.push(build_clone3_filter()?);
    }
    if TARGET_ARCH == TargetArch::x86_64 {
        filters.push(build_x32_filter());
    }
    Ok(filters)
}

/// Denies creating unix or vsock sockets (which
/// reach host services such as the Docker daemon or an SSH agent through any
/// socket file visible in the sandbox) and `io_uring`, which makes sockets
/// without `socket`; and making a user or mount namespace, joining one, or
/// changing a mount tree, which would undo the write denies.
///
/// `socketpair` stays allowed: its sockets are anonymous and runtimes such as
/// libuv rely on it for child process stdio.
fn build_deny_filter(options: SeccompOptions) -> anyhow::Result<BpfProgram> {
    compile(deny_rules(options)?, SeccompAction::Errno(libc::EPERM as u32))
}

fn deny_rules(options: SeccompOptions) -> anyhow::Result<BTreeMap<i64, Vec<SeccompRule>>> {
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();

    rules.insert(
        libc::SYS_ioctl,
        DENIED_TTY_IOCTLS
            .iter()
            .map(|&request| {
                let condition = SeccompCondition::new(1, SeccompCmpArgLen::Dword, SeccompCmpOp::Eq, request)?;
                Ok(SeccompRule::new(vec![condition])?)
            })
            .collect::<anyhow::Result<_>>()?,
    );

    if !options.allow_nested_namespaces {
        rules.extend(NAMESPACE_SYSCALLS.iter().map(|&nr| (nr, Vec::new())));
        rules.insert(libc::SYS_unshare, flag_rules(DENIED_UNSHARE_FLAGS)?);
        rules.insert(libc::SYS_clone, flag_rules(DENIED_CLONE_FLAGS)?);
    }

    if !options.allow_unix_sockets {
        rules.extend(IO_URING_SYSCALLS.iter().map(|&nr| (nr, Vec::new())));
        rules.insert(
            libc::SYS_socket,
            DENIED_SOCKET_DOMAINS
                .iter()
                .map(|&domain| arg0_rule(SeccompCmpOp::Eq, domain))
                .collect::<anyhow::Result<_>>()?,
        );
    }

    Ok(rules)
}

/// `clone3` passes its flags through a pointer that seccomp cannot inspect.
/// Returning `ENOSYS` makes libc fall back to `clone`, where the deny filter
/// can check the namespace flags. `EPERM` would instead break thread creation.
fn build_clone3_filter() -> anyhow::Result<BpfProgram> {
    let rules = BTreeMap::from([(libc::SYS_clone3, Vec::new())]);
    compile(rules, SeccompAction::Errno(libc::ENOSYS as u32))
}

/// x32 syscalls share the x86_64 arch token but set `X32_SYSCALL_BIT` in the
/// syscall number, so they never match the deny rules above. Rejecting the
/// whole ABI keeps those rules from being bypassed on kernels with x32
/// enabled.
fn build_x32_filter() -> BpfProgram {
    let load = (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16;
    let jump_eq = (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16;
    let jump_ge = (libc::BPF_JMP | libc::BPF_JGE | libc::BPF_K) as u16;
    let ret = (libc::BPF_RET | libc::BPF_K) as u16;
    let instruction = |code, jt, jf, k| sock_filter { code, jt, jf, k };
    vec![
        instruction(load, 0, 0, SECCOMP_DATA_ARCH_OFFSET),
        instruction(jump_eq, 0, 3, AUDIT_ARCH_X86_64),
        instruction(load, 0, 0, SECCOMP_DATA_NR_OFFSET),
        instruction(jump_ge, 0, 1, X32_SYSCALL_BIT),
        instruction(ret, 0, 0, libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32),
        instruction(ret, 0, 0, libc::SECCOMP_RET_ALLOW),
    ]
}

fn flag_rules(flags: &[libc::c_int]) -> anyhow::Result<Vec<SeccompRule>> {
    flags
        .iter()
        .map(|&flag| arg0_rule(SeccompCmpOp::MaskedEq(flag as u64), flag))
        .collect()
}

fn arg0_rule(op: SeccompCmpOp, value: libc::c_int) -> anyhow::Result<SeccompRule> {
    let condition = SeccompCondition::new(0, SeccompCmpArgLen::Dword, op, value as u64)?;
    Ok(SeccompRule::new(vec![condition])?)
}

fn compile(rules: BTreeMap<i64, Vec<SeccompRule>>, match_action: SeccompAction) -> anyhow::Result<BpfProgram> {
    let filter = SeccompFilter::new(rules, SeccompAction::Allow, match_action, TARGET_ARCH)?;
    Ok(filter.try_into()?)
}

fn serialize_program(program: &BpfProgram) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(program.len() * 8);
    for instruction in program {
        bytes.extend_from_slice(&instruction.code.to_ne_bytes());
        bytes.push(instruction.jt);
        bytes.push(instruction.jf);
        bytes.extend_from_slice(&instruction.k.to_ne_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests;
