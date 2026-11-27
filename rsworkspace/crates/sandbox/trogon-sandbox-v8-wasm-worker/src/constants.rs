use std::time::Duration;

pub(crate) const BOOTSTRAP_NAME: &str = "sandbox:bootstrap.js";

/// Replaces the built-in `console`, which would write to the host's real stdout and stderr,
/// and removes the `Deno` global so guest code has no path back to the host.
pub(crate) const BOOTSTRAP_SCRIPT: &str = r#"
(() => {
  const { op_captured_print } = Deno.core.ops;
  function stringify(...args) {
    return args.map((a) => {
      if (typeof a === "string") return a;
      try {
        return JSON.stringify(a);
      } catch {
        return String(a);
      }
    }).join(" ");
  }
  const sandboxConsole = {
    log: (...args) => op_captured_print(stringify(...args) + "\n", false),
    debug: (...args) => op_captured_print(stringify(...args) + "\n", false),
    info: (...args) => op_captured_print(stringify(...args) + "\n", false),
    warn: (...args) => op_captured_print(stringify(...args) + "\n", true),
    error: (...args) => op_captured_print(stringify(...args) + "\n", true),
  };
  globalThis.console = sandboxConsole;
  delete globalThis.Deno;
})();
"#;

pub(crate) const BYTES_PER_MIB: usize = 1024 * 1024;

pub(crate) const CANCELLED_EXIT_CODE: i32 = -1;

pub(crate) const CAPTURE_BUFFER_BYTES: usize = 4 * 1024 * 1024;

/// First argv element of a `run_command` against a persistent runtime; the second is the JS
/// source to evaluate.
pub const EVAL_COMMAND: &str = "eval";

/// The environment variable that names the guest kind when the image does not.
pub(crate) const GUEST_KIND_ENV: &str = "V8_WASM_GUEST";

pub(crate) const GUEST_SCRIPT: &str = "sandbox:guest/index.js";

/// The queue group that spreads lease dispatches across the workers of this provider kind.
pub const LEASE_QUEUE_GROUP: &str = "trogon-sandbox-provider-v8_wasm";

/// Most early cancels remembered at once; cancels for leases that never arrive would otherwise
/// accumulate.
pub(crate) const MAX_EARLY_CANCELS: usize = 1024;

/// Most persistent runtimes a worker keeps at once. Each holds up to its lease's heap
/// ceiling, so this bounds the worker's total guest memory.
pub const MAX_PERSISTENT_RUNTIMES: usize = 32;

pub(crate) const MEMORY_LIMIT_MESSAGE: &str = "memory limit exceeded";

/// Set to `true` or `1` in `spec.env` to keep the JS runtime alive after `index.js` ran, so later
/// `run_command` calls on the lease share its globals until the lease is released. Off by
/// default: a lease then runs its guest once and reclaims the isolate.
pub const PERSISTENT_RUNTIME_ENV: &str = "V8_WASM_PERSISTENT_RUNTIME";

pub(crate) const RETIRED_MESSAGE: &str = "runtime is retired";

pub(crate) const SESSION_MEMORY_MESSAGE: &str = "memory limit exceeded; the runtime was retired";

pub(crate) const SESSION_SCRIPT: &str = "sandbox:guest/session.js";

pub(crate) const START_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) const TERMINATE_RETRY: Duration = Duration::from_millis(5);

pub(crate) const TRAP_RETRY: Duration = Duration::from_millis(5);

/// Commands queued for a persistent runtime before callers wait for it.
pub(crate) const SESSION_REQUEST_CAPACITY: usize = 64;
