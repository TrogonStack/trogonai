//! Needs root, `ip`, `nft` and `python3`. Run with
//! `sudo -E cargo test -p trogon-sandbox-firecracker-worker guest_guard -- --ignored`.
use super::*;
use std::io::Write;
use std::process::{Command, Stdio};

const NS: &str = "sbfc-guardtest";
const GOOD_MAC: &str = "06:00:ac:10:00:02";
const BAD_MAC: &str = "06:00:ac:10:00:99";

fn run(program: &str, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let out = Command::new(program).args(args).output()?;
    assert!(
        out.status.success(),
        "{program} {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?)
}

fn ns(args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let mut full = vec!["netns", "exec", NS];
    full.extend_from_slice(args);
    run("ip", &full)
}

fn send(source_mac: &str, ether_type: u16) -> Result<(), Box<dyn std::error::Error>> {
    let script = format!(
        "import socket,binascii\n\
         s=socket.socket(socket.AF_PACKET,socket.SOCK_RAW)\n\
         s.bind(('vb',0))\n\
         mac=lambda m:binascii.unhexlify(m.replace(':',''))\n\
         s.send(mac('{GOOD_MAC}')+mac('{source_mac}')+({ether_type}).to_bytes(2,'big')+bytes(46))\n"
    );
    ns(&["python3", "-c", &script])?;
    Ok(())
}

fn drops() -> Result<u64, Box<dyn std::error::Error>> {
    Ok(parse_drop_count(&ns(&[
        "nft",
        "-j",
        "list",
        "counter",
        "netdev",
        TABLE,
        SPOOF_COUNTER,
    ])?)?)
}

fn exercise() -> Result<(), Box<dyn std::error::Error>> {
    ns(&["ip", "link", "add", "va", "type", "veth", "peer", "name", "vb"])?;
    ns(&["ip", "link", "set", "va", "address", GOOD_MAC, "up"])?;
    ns(&["ip", "link", "set", "vb", "up"])?;
    let mut nft = Command::new("ip")
        .args(["netns", "exec", NS, "nft", "-f", "-"])
        .stdin(Stdio::piped())
        .spawn()?;
    nft.stdin
        .take()
        .ok_or("value missing")?
        .write_all(l2_ruleset("va", GOOD_MAC).as_bytes())?;
    assert!(nft.wait()?.success());

    assert_eq!(drops()?, 0);
    send(GOOD_MAC, 0x0800)?;
    assert_eq!(drops()?, 0, "a frame from the lease MAC must pass");
    send(BAD_MAC, 0x0800)?;
    assert_eq!(drops()?, 1, "a forged source MAC must be dropped");
    send(GOOD_MAC, 0x8100)?;
    assert_eq!(drops()?, 2, "a VLAN-tagged frame must be dropped");
    Ok(())
}

#[test]
#[ignore = "requires root, iproute2, nftables and python3"]
fn forged_frames_move_the_drop_counter_and_genuine_ones_do_not() -> Result<(), Box<dyn std::error::Error>> {
    let _ = Command::new("ip").args(["netns", "delete", NS]).output();
    run("ip", &["netns", "add", NS])?;
    let outcome = exercise();
    let _ = Command::new("ip").args(["netns", "delete", NS]).output();
    outcome?;
    Ok(())
}
