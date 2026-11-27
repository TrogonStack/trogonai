use crate::isolation::constants::{SYS_MOUNT_SETATTR, SYS_OPEN_TREE_ATTR, UNSHARE_EMPTY_MNTNS};
use clap::Parser;

use super::*;

#[test]
fn x32_filter_rejects_syscalls_with_the_x32_bit_on_x86_64_only() -> Result<(), Box<dyn std::error::Error>> {
    let program = serialize_program(&build_x32_filter());
    assert_eq!(program.len(), 6 * 8);
    let k_at = |index: usize| {
        let start = index * 8 + 4;
        u32::from_ne_bytes([
            program[start],
            program[start + 1],
            program[start + 2],
            program[start + 3],
        ])
    };
    assert_eq!(k_at(1), AUDIT_ARCH_X86_64);
    assert_eq!(k_at(3), X32_SYSCALL_BIT);
    assert_eq!(k_at(4), libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32);
    assert_eq!(k_at(5), libc::SECCOMP_RET_ALLOW);
    Ok(())
}

#[test]
fn mount_tree_attribute_syscalls_have_the_unified_numbers() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(SYS_MOUNT_SETATTR, 442);
    assert_eq!(SYS_OPEN_TREE_ATTR, 467);
    assert_eq!(libc::SYS_mount_setattr, SYS_MOUNT_SETATTR);
    Ok(())
}

#[test]
fn every_mount_tree_call_is_denied() -> Result<(), Box<dyn std::error::Error>> {
    for nr in [
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_pivot_root,
        libc::SYS_open_tree,
        libc::SYS_move_mount,
        libc::SYS_fsopen,
        libc::SYS_fsconfig,
        libc::SYS_fsmount,
        libc::SYS_fspick,
        SYS_MOUNT_SETATTR,
        SYS_OPEN_TREE_ATTR,
    ] {
        assert!(NAMESPACE_SYSCALLS.contains(&nr), "syscall {nr} is not denied");
    }
    Ok(())
}

#[test]
fn denied_syscalls_are_listed_once() -> Result<(), Box<dyn std::error::Error>> {
    let all: Vec<_> = NAMESPACE_SYSCALLS.iter().chain(IO_URING_SYSCALLS).collect();
    let unique: std::collections::BTreeSet<_> = all.iter().collect();
    assert_eq!(unique.len(), all.len());
    Ok(())
}

fn denied(options: SeccompOptions) -> Result<std::collections::BTreeSet<i64>, Box<dyn std::error::Error>> {
    Ok(deny_rules(options)?.into_keys().collect())
}

fn nested_namespaces() -> SeccompOptions {
    SeccompOptions {
        allow_nested_namespaces: true,
        ..SeccompOptions::default()
    }
}

fn unix_sockets() -> SeccompOptions {
    SeccompOptions {
        allow_unix_sockets: true,
        ..SeccompOptions::default()
    }
}

#[test]
fn the_default_filter_denies_the_namespace_io_uring_and_socket_calls_and_nothing_else()
-> Result<(), Box<dyn std::error::Error>> {
    let mut expected: std::collections::BTreeSet<i64> =
        NAMESPACE_SYSCALLS.iter().chain(IO_URING_SYSCALLS).copied().collect();
    expected.extend([libc::SYS_unshare, libc::SYS_clone, libc::SYS_socket, libc::SYS_ioctl]);
    assert_eq!(denied(SeccompOptions::default())?, expected);
    assert_eq!(
        build_sandbox_programs(SeccompOptions::default())?.len(),
        2 + usize::from(TARGET_ARCH == TargetArch::x86_64)
    );
    Ok(())
}

#[test]
fn unshare_and_clone_are_refused_only_for_user_and_mount_namespaces() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        DENIED_UNSHARE_FLAGS,
        [libc::CLONE_NEWUSER, libc::CLONE_NEWNS, UNSHARE_EMPTY_MNTNS]
    );
    assert_eq!(DENIED_CLONE_FLAGS, [libc::CLONE_NEWUSER, libc::CLONE_NEWNS]);
    assert_eq!(UNSHARE_EMPTY_MNTNS, libc::CLONE_PARENT_SETTID);
    Ok(())
}

#[test]
fn allowing_unix_sockets_lifts_the_socket_rule_and_io_uring() -> Result<(), Box<dyn std::error::Error>> {
    let all = denied(SeccompOptions::default())?;
    let lifted = denied(unix_sockets())?;
    let mut expected: std::collections::BTreeSet<i64> = IO_URING_SYSCALLS.iter().copied().collect();
    expected.insert(libc::SYS_socket);
    assert_eq!(
        all.difference(&lifted)
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        expected
    );
    Ok(())
}

#[test]
fn allowing_nested_namespaces_lifts_the_namespace_group_and_clone3() -> Result<(), Box<dyn std::error::Error>> {
    let all = denied(SeccompOptions::default())?;
    let lifted = denied(nested_namespaces())?;
    let mut expected: std::collections::BTreeSet<i64> = NAMESPACE_SYSCALLS.iter().copied().collect();
    expected.extend([libc::SYS_unshare, libc::SYS_clone]);
    assert_eq!(
        all.difference(&lifted)
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        expected
    );
    assert!(lifted.contains(&libc::SYS_socket));
    assert_eq!(
        build_sandbox_programs(nested_namespaces())?.len(),
        1 + usize::from(TARGET_ARCH == TargetArch::x86_64)
    );
    Ok(())
}

#[test]
fn allowing_both_still_refuses_terminal_input_injection() -> Result<(), Box<dyn std::error::Error>> {
    let both = SeccompOptions {
        allow_unix_sockets: true,
        allow_nested_namespaces: true,
    };
    assert_eq!(denied(both)?, std::collections::BTreeSet::from([libc::SYS_ioctl]));
    assert_eq!(
        build_sandbox_programs(both)?.len(),
        1 + usize::from(TARGET_ARCH == TargetArch::x86_64)
    );
    Ok(())
}

#[test]
fn terminal_input_injection_ioctls_are_the_ones_refused() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(DENIED_TTY_IOCTLS, [0x5412, 0x541c]);
    Ok(())
}

#[test]
fn the_linux_options_become_flags_and_back() -> Result<(), Box<dyn std::error::Error>> {
    #[derive(Parser)]
    struct Probe {
        #[command(flatten)]
        seccomp: SeccompOptions,
    }

    let options = SeccompOptions::of(&LinuxOptions {
        allow_all_unix_sockets: true.into(),
        allow_nested_user_namespaces: true.into(),
        ..LinuxOptions::default()
    });
    assert_eq!(options.flags(), ["--allow-unix-sockets", "--allow-nested-namespaces"]);
    let parsed = Probe::try_parse_from(std::iter::once("probe".to_string()).chain(options.flags()))?;
    assert_eq!(parsed.seccomp, options);
    assert!(SeccompOptions::default().flags().is_empty());
    Ok(())
}
