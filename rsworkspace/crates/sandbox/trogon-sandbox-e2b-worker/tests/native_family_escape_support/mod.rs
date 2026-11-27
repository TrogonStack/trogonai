// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::process::Command;
use std::time::Duration;

use crate::ensure_support::{Boxed, Context};
use crate::native_command_support::{Binding, Guest};

const MARKER: &str = "e2b-native-family-escape";
const ROOT: &str = r#"middle=$1; leaf=$2; marker=$3
setsid bash -c "$middle" bash "$leaf" "$marker" </dev/null >/dev/null 2>&1 &
wait "$!" || exit 1
n=0; while [ ! -s "$marker" ] && [ "$n" -lt 500 ]; do sleep 0.01; n=$((n+1)); done
[ -s "$marker" ] || exit 1
sleep 300 & wait
"#;
const MIDDLE: &str = r#"leaf=$1; marker=$2
bash -c "$leaf" bash "$marker" </dev/null >/dev/null 2>&1 &
exit 0
"#;
const LEAF: &str = r#"p=$BASHPID; marker=$1
IFS= read -r line <"/proc/$p/stat" || exit 1
fields=${line##*) }; set -- $fields; shift 19
printf '%s %s\n' "$p" "$1" >"$marker.tmp" || exit 1
mv -- "$marker.tmp" "$marker" || exit 1
exec env -i /bin/sleep 300
"#;
const PROBE: &str = r#"p=$1
IFS= read -r line 2>/dev/null <"/proc/$p/stat" || exit 44
fields=${line##*) }; set -- $fields
state=$1; parent=$2; group=$3; session=$4; shift 19; started=$1
bytes=$(wc -c <"/proc/$p/environ") || exit 1
middle=0
if IFS= read -r line 2>/dev/null <"/proc/$session/stat"; then
  fields=${line##*) }; set -- $fields
  [ "$1" = Z ] || middle=1
fi
printf '%s %s %s %s %s %s %s\n' "$state" "$parent" "$group" "$session" "$started" "$bytes" "$middle"
"#;

pub fn command(guest: &Guest) -> Vec<String> {
    let marker = guest.path(MARKER).display().to_string();
    ["bash", "-c", ROOT, "bash", MIDDLE, LEAF, marker.as_str()]
        .map(str::to_owned)
        .to_vec()
}

#[derive(Clone, Copy)]
pub struct Pinned {
    pub pid: u32,
    pub started: u64,
}

pub struct Observation {
    pub alive: bool,
    pub empty_environment: bool,
    pub separate_session: bool,
    pub orphaned: bool,
}

pub fn observe(pinned: Pinned, original: Binding) -> Result<Observation, Boxed> {
    let output = Command::new("bash")
        .args(["-c", PROBE, "bash", &pinned.pid.to_string()])
        .output()?;
    if output.status.code() == Some(44) {
        return Ok(Observation {
            alive: false,
            empty_environment: false,
            separate_session: false,
            orphaned: false,
        });
    }
    assert!(output.status.success(), "exact escaped native identity probe failed");
    let text = std::str::from_utf8(&output.stdout)?;
    let fields = text.split_whitespace().collect::<Vec<_>>();
    assert!(fields.len() == 7, "escaped native probe returned malformed metadata");
    let parent = fields[1].parse::<u32>()?;
    let group = fields[2].parse::<u32>()?;
    let session = fields[3].parse::<u32>()?;
    assert!(
        fields[4].parse::<u64>()? == pinned.started,
        "escaped native PID changed identity instead of preserving the pinned process"
    );
    Ok(Observation {
        alive: fields[0] != "Z",
        empty_environment: fields[5].parse::<usize>()? == 0,
        separate_session: group == session && group != original.pid,
        orphaned: parent != session && fields[6] == "0",
    })
}

pub async fn wait(guest: &Guest, original: Binding) -> Result<Pinned, Boxed> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(marker) = guest.effect(MARKER)? {
                assert!(marker.len() <= 64, "escaped identity marker exceeded its bound");
                let text = std::str::from_utf8(&marker)?;
                let fields = text.split_whitespace().collect::<Vec<_>>();
                assert!(fields.len() == 2, "escaped identity marker is malformed");
                let pinned = Pinned {
                    pid: fields[0].parse()?,
                    started: fields[1].parse()?,
                };
                assert!(
                    pinned.pid > 1 && pinned.started > 0 && pinned.pid != original.pid,
                    "escaped native child must have an independent exact identity"
                );
                let observed = observe(pinned, original)?;
                if observed.alive && observed.empty_environment && observed.separate_session && observed.orphaned {
                    return Ok::<_, Boxed>(pinned);
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("actual double-forked new-session environment-empty native child did not become observable")?
}

/// Ends the escaped child, which no cleanup of the native guest reaches.
pub fn kill(pinned: Pinned) -> Result<(), Boxed> {
    Command::new("kill")
        .args(["-KILL", "--", &pinned.pid.to_string()])
        .output()?;
    Ok(())
}
