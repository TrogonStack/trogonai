use std::path::{Path, PathBuf};

use trogon_sandbox_egress_proxy::ingress::ListenAddress;
use trogon_sandbox_protocol::RegistryHandoff;
use trogon_std::env::SystemEnv;

use super::{CaFilePaths, LeaseProxyExtras, LeaseProxyToken, ProxyPaths, proxy_command};

fn paths() -> ProxyPaths {
    ProxyPaths {
        policy_file: PathBuf::from("/p/policy.json"),
        ca_file: PathBuf::from("/p/ca.pem"),
        listen: ListenAddress::Unix(PathBuf::from("/p/proxy.sock")),
    }
}

#[test]
fn the_proxy_command_carries_only_a_descriptor_number_for_secrets() -> Result<(), Box<dyn std::error::Error>> {
    let secret = format!("tok_{}", "realvalue".repeat(2));
    let token = LeaseProxyToken::parse("abc123")?;
    let extras = LeaseProxyExtras {
        secrets: Some(RegistryHandoff::from_wire(secret.clone())),
        upstream_ca_bundle: None,
        broker: None,
        ca: None,
    };
    let command = proxy_command(Path::new("/bin/proxyd"), &paths(), &token, &extras, Some(7), &SystemEnv);
    let std_command = command.as_std();
    let mut seen = format!("{:?}", std_command.get_program());
    for arg in std_command.get_args() {
        seen.push_str(&format!(" {arg:?}"));
    }
    for (key, value) in std_command.get_envs() {
        seen.push_str(&format!(" {key:?}={value:?}"));
    }
    assert!(!seen.contains("realvalue"), "{seen}");
    assert!(seen.contains("\"EGRESS_PROXY_SECRETS_FD\"=Some(\"7\")"), "{seen}");
    assert!(!format!("{extras:?}").contains("realvalue"));
    Ok(())
}

#[test]
fn a_supplied_ca_reaches_the_proxy_as_file_paths() -> Result<(), Box<dyn std::error::Error>> {
    let token = LeaseProxyToken::parse("abc123")?;
    let extras = LeaseProxyExtras {
        ca: Some(CaFilePaths {
            cert: PathBuf::from("/ca/cert.pem"),
            key: PathBuf::from("/ca/key.pem"),
        }),
        ..LeaseProxyExtras::default()
    };
    let command = proxy_command(Path::new("/bin/proxyd"), &paths(), &token, &extras, None, &SystemEnv);
    let envs: Vec<_> = command
        .as_std()
        .get_envs()
        .map(|(key, value)| format!("{key:?}={value:?}"))
        .collect();
    assert!(envs.contains(&"\"EGRESS_PROXY_CA_CERT_FILE\"=Some(\"/ca/cert.pem\")".to_string()));
    assert!(envs.contains(&"\"EGRESS_PROXY_CA_KEY_FILE\"=Some(\"/ca/key.pem\")".to_string()));
    Ok(())
}
