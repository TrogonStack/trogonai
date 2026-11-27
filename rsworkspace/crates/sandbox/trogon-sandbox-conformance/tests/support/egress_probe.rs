//! Runs the suite's real Python connect probe against a scripted socket, so each way a connection can fail is
//! observed without touching the network.

use std::time::Duration;

use serde::Deserialize;
use trogon_sandbox_contract::{CommandOutput, CommandTimeoutStatus, OutputTruncation, RunCommandRequest};
use trogon_std::env::{ReadEnv, SystemEnv};

use super::wire::Sendable;

/// What the scripted socket does when the probe connects.
#[derive(Clone, Copy, Debug)]
pub enum Scenario {
    Connected,
    /// A socket deadline that is an `OSError` without an errno, as older interpreters raise it.
    LegacyTimeout,
    BuiltinTimeout,
    UnknownOsError,
    ArbitraryError,
}

impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::LegacyTimeout => "legacy_timeout",
            Self::BuiltinTimeout => "builtin_timeout",
            Self::UnknownOsError => "unknown_os_error",
            Self::ArbitraryError => "arbitrary_error",
        }
    }
}

/// What the scripted socket saw the probe do.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proof {
    pub connect_calls: u8,
    pub close_calls: u8,
    pub timeout_seconds: u8,
    pub endpoint_preserved: bool,
    pub socket_timeout_raised: bool,
    pub builtin_timeout_raised: bool,
    pub errno_none: bool,
}

const GUEST: &str = r#"import errno,json,os,socket,sys
scenario=os.environ['EGRESS_PYTHON_SCENARIO']
proof={'connect_calls':0,'close_calls':0,'timeout_seconds':0,'endpoint_preserved':False,
       'socket_timeout_raised':False,'builtin_timeout_raised':False,'errno_none':False}
class LegacySocketTimeout(OSError):
 pass
assert not issubclass(LegacySocketTimeout,TimeoutError)
socket.timeout=LegacySocketTimeout
class Connection:
 def __init__(self,family,kind):
  assert family==socket.AF_INET and kind==socket.SOCK_STREAM
 def settimeout(self,seconds):
  assert seconds==3
  proof['timeout_seconds']=seconds
 def connect(self,endpoint):
  assert tuple(endpoint)==('1.1.1.1',443)
  proof['connect_calls']+=1
  proof['endpoint_preserved']=True
  if scenario=='connected':
   return
  if scenario=='legacy_timeout':
   error=LegacySocketTimeout('scripted socket deadline')
   assert error.errno is None
  elif scenario=='builtin_timeout':
   error=TimeoutError('scripted built-in deadline')
  elif scenario=='unknown_os_error':
   error=OSError(999,'scripted unknown OS failure')
  elif scenario=='arbitrary_error':
   error=RuntimeError('scripted arbitrary failure')
  else:
   raise AssertionError('unknown scenario')
  proof['socket_timeout_raised']=isinstance(error,socket.timeout)
  proof['builtin_timeout_raised']=isinstance(error,TimeoutError)
  proof['errno_none']=getattr(error,'errno',None) is None
  raise error
 def close(self):
  proof['close_calls']+=1
def no_dns(*args,**kwargs):
 raise AssertionError('a literal endpoint does not resolve names')
socket.socket=Connection
socket.getaddrinfo=no_dns
assert sys.argv[1]=='-c'
source=sys.argv[2]
sys.argv=['-c',*sys.argv[3:]]
try:
 exec(compile(source,'<egress-probe>','exec'),{'__name__':'__main__'})
finally:
 with open(os.environ['EGRESS_PYTHON_PROOF'],'w') as output:
  json.dump(proof,output)
"#;

/// Runs the probe `request` carries under `scenario` and returns its output and what the socket saw.
pub async fn execute(request: &RunCommandRequest, scenario: Scenario) -> Sendable<(CommandOutput, Proof)> {
    if request.command.first().map(String::as_str) != Some("python3") {
        return Err("the scripted socket runs only the Python probe".into());
    }
    if request.timeout_seconds != 20 || request.max_output_bytes != Some(4096) {
        return Err("the probe asks for a 20 second deadline and 4096 bytes of output".into());
    }
    let dir = tempfile::tempdir()?;
    let proof_path = dir.path().join("proof.json");
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new("python3")
            .args(["-c", GUEST])
            .args(&request.command[1..])
            .env_clear()
            .env("PATH", SystemEnv.var("PATH").unwrap_or_default())
            .env("EGRESS_PYTHON_SCENARIO", scenario.name())
            .env("EGRESS_PYTHON_PROOF", &proof_path)
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    let proof = serde_json::from_slice(&std::fs::read(&proof_path)?)?;
    Ok((
        CommandOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
            timed_out: CommandTimeoutStatus::WithinDeadline,
            truncated: OutputTruncation::Complete,
        },
        proof,
    ))
}
