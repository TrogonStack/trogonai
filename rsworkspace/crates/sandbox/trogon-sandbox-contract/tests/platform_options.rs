use serde::de::DeserializeOwned;
use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{
    AllUnixSocketsAccess, AppleEventsAccess, ContainerOverrides, ExtraCapability, ExtraCapabilityError,
    FilesystemEnforcement, FilesystemOptions, GitConfigAccess, LinuxOptions, LiteralPath, LocalBindingAccess,
    MachServiceName, MachServiceNameError, MacosOptions, MandatoryDenySearchDepth, MandatoryDenySearchDepthError,
    NestedSandboxMode, NestedUserNamespaceAccess, PidsLimit, PidsLimitError, PlatformOptions, ProtectedNamesAnchor,
    PtyAccess, RootfsOverride, ShmSizeMib, ShmSizeMibError, TrustServiceAccess,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn error_of<T, E>(result: Result<T, E>) -> Result<E, Box<dyn std::error::Error>> {
    result.err().ok_or_else(|| "expected an error".into())
}

fn rejects<T: DeserializeOwned>(value: serde_json::Value) -> TestResult {
    let from_value = error_of(serde_json::from_value::<T>(value.clone()))?;
    assert_eq!(from_value.classify(), Data);
    let from_str = error_of(serde_json::from_str::<T>(&value.to_string()))?;
    assert_eq!(from_str.classify(), Data);
    Ok(())
}

#[test]
fn a_mach_service_name_is_exact_or_a_trailing_prefix() -> TestResult {
    for raw in ["com.example.service", "com.example.*", "*"] {
        let name = MachServiceName::new(raw)?;
        assert_eq!(name.as_str(), raw);
        assert_eq!(name.to_string(), raw);
        assert_eq!(raw.parse::<MachServiceName>()?, name);
        assert_eq!(MachServiceName::try_from(raw.to_string())?, name);
        assert_eq!(String::from(name.clone()), raw);
        assert_eq!(serde_json::to_value(&name)?, json!(raw));
        assert_eq!(serde_json::from_value::<MachServiceName>(json!(raw))?, name);
    }
    Ok(())
}

#[test]
fn a_mach_service_name_rejects_inner_wildcards_empty_and_control_characters() -> TestResult {
    for raw in ["com.*.example", "com.example.**", "*.example"] {
        assert_eq!(
            error_of(MachServiceName::new(raw))?,
            MachServiceNameError::InvalidWildcard(raw.to_string())
        );
        rejects::<MachServiceName>(json!(raw))?;
    }
    assert_eq!(error_of(MachServiceName::new(""))?, MachServiceNameError::Empty);
    assert_eq!(
        error_of(MachServiceName::new(String::from("com.example\n")))?,
        MachServiceNameError::ControlCharacter('\n')
    );
    rejects::<MachServiceName>(json!(""))?;
    rejects::<MachServiceName>(json!("a\u{7}"))?;
    rejects::<MachServiceName>(json!(3))?;
    for (error, text) in [
        (MachServiceNameError::Empty, "a Mach service name cannot be empty"),
        (
            MachServiceNameError::InvalidWildcard("a*b".into()),
            "`a*b` has a wildcard that is not a single trailing `*`",
        ),
        (
            MachServiceNameError::ControlCharacter('\n'),
            "a Mach service name cannot hold the control character '\\n'",
        ),
    ] {
        assert_eq!(error.to_string(), text);
    }
    Ok(())
}

#[test]
fn the_search_depth_is_bounded() -> TestResult {
    for levels in [0u8, 11, 255] {
        let want = MandatoryDenySearchDepthError {
            min: 1,
            max: 10,
            got: levels,
        };
        assert_eq!(error_of(MandatoryDenySearchDepth::new(levels))?, want);
        assert_eq!(error_of(MandatoryDenySearchDepth::try_from(levels))?, want);
        rejects::<MandatoryDenySearchDepth>(json!(levels))?;
    }
    for levels in [1u8, 3, 10] {
        let depth = MandatoryDenySearchDepth::new(levels)?;
        assert_eq!(depth.get(), levels);
        assert_eq!(u8::from(depth), levels);
        assert_eq!(serde_json::to_value(depth)?, json!(levels));
        assert_eq!(
            serde_json::from_value::<MandatoryDenySearchDepth>(json!(levels))?,
            depth
        );
    }
    assert_eq!(MandatoryDenySearchDepth::DEFAULT.get(), 3);
    assert_eq!(
        MandatoryDenySearchDepthError {
            min: 1,
            max: 10,
            got: 11
        }
        .to_string(),
        "the mandatory deny search depth must be from 1 to 10, not 11"
    );
    Ok(())
}

#[test]
fn a_pids_limit_and_a_shm_size_are_bounded() -> TestResult {
    for count in [0u32, PidsLimit::MAX + 1] {
        let want = PidsLimitError {
            max: PidsLimit::MAX,
            got: count,
        };
        assert_eq!(error_of(PidsLimit::new(count))?, want);
        assert_eq!(error_of(PidsLimit::try_from(count))?, want);
        rejects::<PidsLimit>(json!(count))?;
    }
    for count in [1, 512, PidsLimit::MAX] {
        let limit = PidsLimit::new(count)?;
        assert_eq!(limit.get(), count);
        assert_eq!(u32::from(limit), count);
        assert_eq!(
            serde_json::from_value::<PidsLimit>(serde_json::to_value(limit)?)?,
            limit
        );
    }
    for size in [0u32, ShmSizeMib::MAX + 1] {
        let want = ShmSizeMibError {
            max: ShmSizeMib::MAX,
            got: size,
        };
        assert_eq!(error_of(ShmSizeMib::new(size))?, want);
        assert_eq!(error_of(ShmSizeMib::try_from(size))?, want);
        rejects::<ShmSizeMib>(json!(size))?;
    }
    for size in [1, 1024, ShmSizeMib::MAX] {
        let mebibytes = ShmSizeMib::new(size)?;
        assert_eq!(mebibytes.get(), size);
        assert_eq!(u32::from(mebibytes), size);
        assert_eq!(
            serde_json::from_value::<ShmSizeMib>(serde_json::to_value(mebibytes)?)?,
            mebibytes
        );
    }
    assert_eq!(
        PidsLimitError { max: 65_536, got: 0 }.to_string(),
        "pids_limit must be between 1 and 65536, not 0"
    );
    assert_eq!(
        ShmSizeMibError { max: 8_192, got: 0 }.to_string(),
        "shm_size_mib must be between 1 and 8192, not 0"
    );
    Ok(())
}

#[test]
fn an_extra_capability_is_well_formed_and_never_forbidden() -> TestResult {
    for raw in ["NET_RAW", "SYS_PTRACE", "X1"] {
        let capability = ExtraCapability::new(raw)?;
        assert_eq!(capability.as_str(), raw);
        assert_eq!(capability.to_string(), raw);
        assert_eq!(raw.parse::<ExtraCapability>()?, capability);
        assert_eq!(ExtraCapability::try_from(raw.to_string())?, capability);
        assert_eq!(String::from(capability.clone()), raw);
        assert_eq!(
            serde_json::from_value::<ExtraCapability>(serde_json::to_value(&capability)?)?,
            capability
        );
    }
    for forbidden in ExtraCapability::FORBIDDEN {
        assert_eq!(
            error_of(ExtraCapability::new(String::from(forbidden)))?,
            ExtraCapabilityError::Forbidden(forbidden.to_string())
        );
        rejects::<ExtraCapability>(json!(forbidden))?;
    }
    for malformed in ["", "cap_net_raw", "NET RAW", "CAP-X"] {
        assert_eq!(
            error_of(ExtraCapability::new(malformed))?,
            ExtraCapabilityError::InvalidName(malformed.to_string())
        );
        rejects::<ExtraCapability>(json!(malformed))?;
    }
    assert_eq!(
        ExtraCapabilityError::InvalidName("x".into()).to_string(),
        "`x` is not a valid capability name; use upper case letters, digits and `_`"
    );
    assert_eq!(
        ExtraCapabilityError::Forbidden("ALL".into()).to_string(),
        "capability `ALL` can never be added by an override"
    );
    Ok(())
}

macro_rules! flag {
    ($type:ty, $on:expr, $off:expr, $query:ident) => {
        assert_eq!(<$type>::default(), $off);
        assert!($on.$query());
        assert!(!$off.$query());
        assert_eq!(<$type>::from(true), $on);
        assert_eq!(<$type>::from(false), $off);
        assert!(bool::from($on));
        assert!(!bool::from($off));
        assert_eq!(serde_json::to_value($on)?, json!(true));
        assert_eq!(serde_json::to_value($off)?, json!(false));
        assert_eq!(serde_json::from_value::<$type>(json!(true))?, $on);
        assert_eq!(serde_json::from_value::<$type>(json!(false))?, $off);
        rejects::<$type>(json!("true"))?;
    };
}

#[test]
fn every_flag_is_a_two_variant_enum_on_a_json_boolean() -> TestResult {
    flag!(PtyAccess, PtyAccess::Allowed, PtyAccess::Denied, is_allowed);
    flag!(
        LocalBindingAccess,
        LocalBindingAccess::Allowed,
        LocalBindingAccess::Denied,
        is_allowed
    );
    flag!(
        TrustServiceAccess,
        TrustServiceAccess::Allowed,
        TrustServiceAccess::Denied,
        is_allowed
    );
    flag!(
        AllUnixSocketsAccess,
        AllUnixSocketsAccess::Allowed,
        AllUnixSocketsAccess::Denied,
        is_allowed
    );
    flag!(
        AppleEventsAccess,
        AppleEventsAccess::Allowed,
        AppleEventsAccess::Denied,
        is_allowed
    );
    flag!(
        NestedUserNamespaceAccess,
        NestedUserNamespaceAccess::Allowed,
        NestedUserNamespaceAccess::Denied,
        is_allowed
    );
    flag!(
        NestedSandboxMode,
        NestedSandboxMode::Weaker,
        NestedSandboxMode::Standard,
        is_weaker
    );
    flag!(
        GitConfigAccess,
        GitConfigAccess::Writable,
        GitConfigAccess::Protected,
        is_writable
    );
    flag!(
        FilesystemEnforcement,
        FilesystemEnforcement::Disabled,
        FilesystemEnforcement::Enforced,
        is_disabled
    );
    flag!(
        RootfsOverride,
        RootfsOverride::Writable,
        RootfsOverride::Unchanged,
        is_writable
    );
    Ok(())
}

#[test]
fn the_defaults_serialize_to_nothing_and_old_specs_still_read() -> TestResult {
    assert_eq!(serde_json::to_value(PlatformOptions::default())?, json!({}));
    let read: PlatformOptions = serde_json::from_value(json!({}))?;
    assert!(read.is_default());
    assert!(read.macos.is_default());
    assert!(read.linux.is_default());
    assert!(read.filesystem.is_default());
    assert!(read.container.is_default());
    assert!(ProtectedNamesAnchor::default().is_default());
    assert!(!ProtectedNamesAnchor::WorkingDirectory.is_default());
    Ok(())
}

#[test]
fn a_group_alone_shows_only_its_own_fields() -> TestResult {
    let mut options = PlatformOptions::default();
    options.container.writable_rootfs = RootfsOverride::Writable;
    assert!(!options.is_default());
    assert!(!options.container.is_default());
    assert_eq!(
        serde_json::to_value(&options)?,
        json!({ "container": { "writable_rootfs": true } })
    );
    assert_eq!(
        serde_json::to_value(MacosOptions::default())?,
        json!({
            "allow_pty": false,
            "allow_local_binding": false,
            "allow_trust_service": false,
            "allow_all_unix_sockets": false,
            "allow_apple_events": false,
        })
    );
    assert_eq!(
        serde_json::to_value(LinuxOptions::default())?,
        json!({
            "weaker_nested_sandbox": false,
            "allow_all_unix_sockets": false,
            "allow_nested_user_namespaces": false,
        })
    );
    assert_eq!(
        serde_json::to_value(FilesystemOptions::default())?,
        json!({ "allow_git_config": false, "disabled": false })
    );
    assert_eq!(
        serde_json::to_value(ContainerOverrides::default())?,
        json!({ "writable_rootfs": false })
    );
    Ok(())
}

#[test]
fn full_options_have_an_exact_wire_shape_and_round_trip() -> TestResult {
    let options = PlatformOptions {
        macos: MacosOptions {
            allow_pty: PtyAccess::Allowed,
            allow_local_binding: LocalBindingAccess::Allowed,
            allow_mach_lookup: vec![MachServiceName::new("com.example.*")?],
            allow_trust_service: TrustServiceAccess::Allowed,
            allow_unix_sockets: vec![LiteralPath::new("/var/run/docker.sock")?],
            allow_all_unix_sockets: AllUnixSocketsAccess::Allowed,
            allow_apple_events: AppleEventsAccess::Allowed,
            protected_names_anchor: ProtectedNamesAnchor::WorkingDirectory,
        },
        linux: LinuxOptions {
            weaker_nested_sandbox: NestedSandboxMode::Weaker,
            allow_all_unix_sockets: AllUnixSocketsAccess::Allowed,
            allow_nested_user_namespaces: NestedUserNamespaceAccess::Allowed,
        },
        filesystem: FilesystemOptions {
            allow_git_config: GitConfigAccess::Writable,
            disabled: FilesystemEnforcement::Disabled,
            mandatory_deny_search_depth: Some(MandatoryDenySearchDepth::new(3)?),
        },
        git_safe_directories: vec![LiteralPath::new("/workspace")?],
        container: ContainerOverrides {
            pids_limit: Some(PidsLimit::new(512)?),
            writable_rootfs: RootfsOverride::Writable,
            extra_capabilities: vec![ExtraCapability::new("NET_RAW")?],
            shm_size_mib: Some(ShmSizeMib::new(1024)?),
        },
    };
    let wire = json!({
        "macos": {
            "allow_pty": true,
            "allow_local_binding": true,
            "allow_mach_lookup": ["com.example.*"],
            "allow_trust_service": true,
            "allow_unix_sockets": ["/var/run/docker.sock"],
            "allow_all_unix_sockets": true,
            "allow_apple_events": true,
            "protected_names_anchor": "working_directory",
        },
        "linux": {
            "weaker_nested_sandbox": true,
            "allow_all_unix_sockets": true,
            "allow_nested_user_namespaces": true,
        },
        "filesystem": {
            "allow_git_config": true,
            "disabled": true,
            "mandatory_deny_search_depth": 3,
        },
        "git_safe_directories": ["/workspace"],
        "container": {
            "pids_limit": 512,
            "writable_rootfs": true,
            "extra_capabilities": ["NET_RAW"],
            "shm_size_mib": 1024,
        },
    });
    assert_eq!(serde_json::to_value(&options)?, wire);
    assert_eq!(serde_json::from_value::<PlatformOptions>(wire)?, options);
    Ok(())
}

#[test]
fn a_protected_names_anchor_uses_snake_case_names() -> TestResult {
    for (anchor, name) in [
        (ProtectedNamesAnchor::WriteRoots, "write_roots"),
        (ProtectedNamesAnchor::WorkingDirectory, "working_directory"),
    ] {
        assert_eq!(serde_json::to_value(anchor)?, json!(name));
        assert_eq!(serde_json::from_value::<ProtectedNamesAnchor>(json!(name))?, anchor);
    }
    rejects::<ProtectedNamesAnchor>(json!("elsewhere"))?;
    Ok(())
}

#[test]
fn bad_nested_values_are_rejected_on_read() -> TestResult {
    for bad in [
        json!({ "macos": { "allow_mach_lookup": ["a.*.b"] } }),
        json!({ "macos": { "allow_unix_sockets": ["relative"] } }),
        json!({ "git_safe_directories": [""] }),
        json!({ "filesystem": { "mandatory_deny_search_depth": 0 } }),
        json!({ "container": { "pids_limit": 0 } }),
        json!({ "container": { "shm_size_mib": 8193 } }),
        json!({ "container": { "extra_capabilities": ["SYS_ADMIN"] } }),
        json!({ "container": { "extra_capabilities": ["lower"] } }),
        json!({ "linux": { "weaker_nested_sandbox": "yes" } }),
    ] {
        rejects::<PlatformOptions>(bad)?;
    }
    Ok(())
}
