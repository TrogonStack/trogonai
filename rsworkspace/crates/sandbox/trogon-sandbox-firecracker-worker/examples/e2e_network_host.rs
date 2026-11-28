//! Exercises the worker's Linux network setup and real packet filtering without KVM.

#[cfg(target_os = "linux")]
use anyhow::{Context, ensure};
#[cfg(target_os = "linux")]
use std::io::Write;
#[cfg(target_os = "linux")]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(target_os = "linux")]
use trogon_sandbox_contract::{LeaseId, NetworkPolicy};
#[cfg(target_os = "linux")]
use trogon_sandbox_firecracker_worker::config::StateDir;
#[cfg(target_os = "linux")]
use trogon_sandbox_firecracker_worker::network::{LeaseNetwork, NetworkConfig};
#[cfg(target_os = "linux")]
use trogon_sandbox_firecracker_worker::vm_dir::VmDir;

#[cfg(target_os = "linux")]
async fn ip(args: &[&str]) -> anyhow::Result<String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new("ip")
            .args(args)
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    ensure!(
        output.status.success(),
        "ip test command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}

#[cfg(target_os = "linux")]
async fn run() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let dir = VmDir::for_lease(&StateDir::new(directory.path()), &LeaseId::new("network"))?;
    dir.create().await?;
    let config = NetworkConfig {
        host_links: "10.231.0.0/24".parse()?,
    };
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let target_port = listener.local_addr()?.port();
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buffer = [0; 8192];
                let _ = stream.read(&mut buffer).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\nConnection: close\r\n\r\nfc-network-ok")
                    .await;
            });
        }
    });
    let policy: NetworkPolicy = serde_json::from_str(
        r#"{"user_defined": {"rules": [], "allow_ranges": ["127.0.0.1/32"], "deny_ranges": [], "opaque_tunnels": "allow"}}"#,
    )?;
    let mut network = LeaseNetwork::prepare(&config, 2, &dir, policy, None, None).await?;
    let namespace_path = network.namespace_path();
    let namespace = namespace_path
        .file_name()
        .and_then(|s| s.to_str())
        .context("invalid namespace")?;
    let guest = format!("sbfc-guest-{}", std::process::id());
    let outcome = async {
        let links: serde_json::Value =
            serde_json::from_str(&ip(&["-n", namespace, "-j", "-d", "link", "show", "tap0"]).await?)?;
        ensure!(
            links[0]["linkinfo"]["info_kind"] == "tun",
            "worker did not create a TAP device"
        );
        let attach = r#"
import errno, fcntl, os, struct, sys
os.setgroups([])
identity = int(sys.argv[2])
os.setgid(identity)
os.setuid(identity)
with open('/proc/self/status') as status:
    capabilities = next(line for line in status if line.startswith('CapEff:'))
assert int(capabilities.split()[1], 16) == 0, capabilities
fd = os.open('/dev/net/tun', os.O_RDWR)
try:
    fcntl.ioctl(fd, 0x400454ca, struct.pack('16sH', b'tap0', 0x1002))
except OSError as error:
    assert sys.argv[1] == 'denied' and error.errno == errno.EPERM, error
else:
    assert sys.argv[1] == 'allowed', 'unassigned TAP allowed a nonroot VMM'
finally:
    os.close(fd)
"#;
        ip(&["netns", "exec", namespace, "python3", "-c", attach, "denied", "200000"]).await?;
        trogon_sandbox_firecracker_worker::network::assign_tap_owner(&namespace_path, 200000, 200000).await?;
        ip(&["netns", "exec", namespace, "python3", "-c", attach, "allowed", "200000"]).await?;
        ip(&["netns", "exec", namespace, "python3", "-c", attach, "denied", "200001"]).await?;
        let assigned: serde_json::Value =
            serde_json::from_str(&ip(&["-n", namespace, "-j", "link", "show", "tap0"]).await?)?;
        ensure!(
            assigned[0]["address"] == links[0]["address"],
            "TAP reassignment changed the snapshot gateway MAC"
        );
        writeln!(
            std::io::stdout(),
            "reserved nonroot VMM can attach TAP; gateway MAC preserved"
        )?;
        ip(&["netns", "add", &guest]).await?;
        // A veth sends real guest packets through the same tap0-name firewall boundary.
        ip(&["-n", namespace, "link", "delete", "tap0"]).await?;
        ip(&["link", "add", "inject0", "type", "veth", "peer", "name", "guest0"]).await?;
        ip(&["link", "set", "inject0", "netns", namespace]).await?;
        ip(&["-n", namespace, "link", "set", "inject0", "name", "tap0"]).await?;
        ip(&["-n", namespace, "address", "add", "172.16.0.1/30", "dev", "tap0"]).await?;
        ip(&["-n", namespace, "link", "set", "tap0", "up"]).await?;
        ip(&["link", "set", "guest0", "netns", &guest]).await?;
        ip(&["-n", &guest, "address", "add", "172.16.0.2/30", "dev", "guest0"]).await?;
        ip(&["-n", &guest, "link", "set", "guest0", "up"]).await?;
        ip(&["-n", &guest, "link", "set", "lo", "up"]).await?;
        ip(&["-n", &guest, "route", "add", "default", "via", "172.16.0.1"]).await?;
        let script = r#"
import base64, http.client, socket, sys, urllib.parse
proxy = urllib.parse.urlsplit(sys.argv[1])
target = int(sys.argv[2])
auth = base64.b64encode((proxy.username + ':' + proxy.password).encode()).decode()
for credentials, expected in [('Basic ' + auth, 200), ('Basic invalid', 407)]:
    connection = http.client.HTTPConnection(proxy.hostname, proxy.port, timeout=5)
    connection.request('GET', 'http://127.0.0.1:%d/' % target, headers={'Proxy-Authorization': credentials})
    response = connection.getresponse()
    assert response.status == expected, (expected, response.status, response.read().decode(errors='replace'))
    if expected == 200:
        assert response.read() == b'fc-network-ok'
    connection.close()
try:
    socket.create_connection((proxy.hostname, target), timeout=2).close()
except socket.timeout:
    pass
else:
    raise AssertionError('direct host service bypassed the proxy firewall')
print('authenticated proxy passed; wrong credentials and direct host access denied')
"#;
        let output = ip(&[
            "netns",
            "exec",
            &guest,
            "python3",
            "-c",
            script,
            &network.proxy_url(),
            &target_port.to_string(),
        ])
        .await?;
        write!(std::io::stdout(), "{output}")?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let guest_cleanup = ip(&["netns", "delete", &guest]).await;
    let network_cleanup = network.shutdown(&dir).await;
    server.abort();
    outcome?;
    guest_cleanup?;
    network_cleanup?;
    ensure!(!namespace_path.exists(), "network namespace survived cleanup");
    ensure!(
        !dir.as_path().join("network.json").exists(),
        "network recovery manifest survived cleanup"
    );
    writeln!(std::io::stdout(), "test-owned namespaces, links and nftables removed")?;
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run().await
}

#[cfg(not(target_os = "linux"))]
fn main() {
    let _ = std::io::Write::write_all(
        &mut std::io::stderr(),
        b"e2e_network_host needs Linux network-administration privileges\n",
    );
}
