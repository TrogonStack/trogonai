use crate::ExecModel;

/// First argv element of a command on an [`ExecModel::JsEval`] worker.
pub(crate) const EVAL: &str = "eval";

/// Tenant every check leases under. A signed request binds it.
pub(crate) const TENANT: &str = "conformance";

pub(crate) const ANY: &[ExecModel] = &[ExecModel::Shell, ExecModel::JsEval, ExecModel::None];
pub(crate) const SHELL: &[ExecModel] = &[ExecModel::Shell];
pub(crate) const JS_EVAL: &[ExecModel] = &[ExecModel::JsEval];
/// Workers whose leases hold state the suite can plant and read back.
pub(crate) const STATEFUL: &[ExecModel] = &[ExecModel::Shell, ExecModel::JsEval];

pub(crate) const TOOL_PREFLIGHT: &str = "if command -v nc >/dev/null 2>&1; then printf 'NC\\n'; \
    elif command -v python3 >/dev/null 2>&1; then \
    python3 -c 'import socket,sys; assert sys.version_info.major == 3; print(\"PYTHON3\")'; \
    else printf 'NO_PROBE\\n'; fi";

pub(crate) const NC_PROBE: &str = "nc -z -w 3 \"$1\" \"$2\" >/dev/null 2>&1; status=$?; \
    case \"$status\" in 0) printf 'CONNECTED\\n';; 1) printf 'DENIED\\n';; *) exit 1;; esac";

pub(crate) const PYTHON_PROBE: &str = r#"import errno,socket,sys
connection=socket.socket(socket.AF_INET6 if ':' in sys.argv[1] else socket.AF_INET,socket.SOCK_STREAM)
connection.settimeout(3)
try:
 try:
  connection.connect((sys.argv[1],int(sys.argv[2])))
 except (TimeoutError,socket.timeout):
  print('DENIED')
 except OSError as error:
  if error.errno not in (errno.EACCES,errno.EPERM,errno.ENETUNREACH,errno.EHOSTUNREACH,errno.ETIMEDOUT,errno.ECONNREFUSED):
   raise
  print('DENIED')
 else:
  print('CONNECTED')
finally:
 connection.close()
"#;
