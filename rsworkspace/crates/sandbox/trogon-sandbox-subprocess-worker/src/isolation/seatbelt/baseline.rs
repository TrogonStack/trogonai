//! What a macOS process needs to start and run at all, none of it a way to
//! the network or to files.

use super::sbpl::{AllowRule, Filter, Operation, SbplError, VnodeType};
use crate::isolation::default_writes::DEFAULT_WRITE_DEVICES;

pub fn baseline_allows() -> Result<Vec<AllowRule>, SbplError> {
    let mut rules = vec![
        AllowRule::new([Operation::ProcessExec], []),
        AllowRule::new([Operation::ProcessFork], []),
        AllowRule::new([Operation::ProcessInfoStar], [Filter::TargetSameSandbox]),
        AllowRule::new([Operation::Signal], [Filter::TargetSameSandbox]),
        AllowRule::new([Operation::MachPrivTaskPort], [Filter::TargetSameSandbox]),
        AllowRule::new([Operation::UserPreferenceRead], []),
        AllowRule::new(
            [Operation::MachLookup],
            MACH_SERVICES
                .iter()
                .map(|name| Filter::global_name(name))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        AllowRule::new(
            [Operation::MachLookup],
            [Filter::global_name("com.apple.SecurityServer")?],
        ),
        AllowRule::new([Operation::IpcPosixShm], []),
        AllowRule::new([Operation::IpcPosixSem], []),
        AllowRule::new(
            [Operation::IokitOpen],
            [
                Filter::iokit_registry_entry_class("IOSurfaceRootUserClient")?,
                Filter::iokit_registry_entry_class("RootDomainUserClient")?,
                Filter::iokit_user_client_class("IOSurfaceSendRight")?,
            ],
        ),
        AllowRule::new([Operation::IokitGetProperties], []),
        AllowRule::new(
            [Operation::SystemSocket],
            [Filter::RequireAll(vec![
                Filter::SocketDomainSystem,
                Filter::SocketProtocol(2),
            ])],
        ),
    ];

    let mut sysctls = SYSCTL_NAMES
        .iter()
        .map(|name| Filter::sysctl_name(name))
        .collect::<Result<Vec<_>, _>>()?;
    sysctls.extend(
        SYSCTL_PREFIXES
            .iter()
            .map(|prefix| Filter::sysctl_name_prefix(prefix))
            .collect::<Result<Vec<_>, _>>()?,
    );
    rules.push(AllowRule::new([Operation::SysctlRead], sysctls));
    rules.push(AllowRule::new(
        [Operation::SysctlWrite],
        [Filter::sysctl_name("kern.tcsm_enable")?],
    ));
    rules.push(AllowRule::new([Operation::DistributedNotificationPost], []));
    rules.push(AllowRule::new(
        [Operation::FileIoctl],
        IOCTL_DEVICES
            .iter()
            .map(|device| Filter::literal(device))
            .collect::<Result<Vec<_>, _>>()?,
    ));
    rules.push(AllowRule::new(
        [Operation::FileIoctl, Operation::FileReadData, Operation::FileWriteData],
        [Filter::RequireAll(vec![
            Filter::literal("/dev/null")?,
            Filter::VnodeType(VnodeType::CharacterDevice),
        ])],
    ));
    rules.push(AllowRule::new(
        [Operation::FileWriteStar],
        DEFAULT_WRITE_DEVICES
            .iter()
            .map(|device| Filter::literal(device))
            .collect::<Result<Vec<_>, _>>()?,
    ));
    Ok(rules)
}

#[cfg(test)]
mod tests;
use super::constants::{IOCTL_DEVICES, MACH_SERVICES, SYSCTL_NAMES, SYSCTL_PREFIXES};
