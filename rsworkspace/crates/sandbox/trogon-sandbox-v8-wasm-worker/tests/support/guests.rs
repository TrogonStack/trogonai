//! Hand-written WASM components used as guests. Each exports `wasi:cli/run` and returns
//! success unless noted.

const RUN_EXPORT: &str = r#"
  (core instance $guest (instantiate $guest_module (with "mem" (instance $mem)) (with "host" (instance $host))))
  (func $run (result (result)) (canon lift (core func $guest "run")))
  (instance $cli (export "run" (func $run)))
  (export "wasi:cli/run@0.2.12" (instance $cli))
"#;

const MEMORY_MODULE: &str = r#"
  (core module $mem_module
    (memory (export "memory") 2 2)
    (global $heap (mut i32) (i32.const 4096))
    (data (i32.const 8) "hello from wasm\n")
    (func (export "realloc") (param i32 i32 i32 i32) (result i32) (local $p i32)
      (local.set $p (global.get $heap))
      (global.set $heap (i32.add (local.get $p) (i32.and (i32.add (local.get 3) (i32.const 7)) (i32.const -8))))
      (local.get $p)))
  (core instance $mem (instantiate $mem_module))
"#;

fn parse(text: &str) -> Vec<u8> {
    wat::parse_str(text).unwrap_or_default()
}

/// A guest whose `run` executes `body` and returns its `i32` (0 is success).
pub fn simple(body: &str) -> Vec<u8> {
    parse(&format!(
        r#"(component
  (core module $guest_module (func (export "run") (result i32) {body}))
  (core instance $guest (instantiate $guest_module))
  (func $run (result (result)) (canon lift (core func $guest "run")))
  (instance $cli (export "run" (func $run)))
  (export "wasi:cli/run@0.2.12" (instance $cli)))"#
    ))
}

pub fn succeeds() -> Vec<u8> {
    simple("i32.const 0")
}

pub fn fails_with_exit_status() -> Vec<u8> {
    simple("i32.const 1")
}

pub fn traps() -> Vec<u8> {
    simple("unreachable")
}

pub fn spins() -> Vec<u8> {
    simple("(loop $spin (br $spin)) i32.const 0")
}

/// Returns success only when growing linear memory by 100 pages (6.4 MiB) is refused.
pub fn succeeds_only_if_memory_growth_is_denied() -> Vec<u8> {
    parse(
        r#"(component
  (core module $guest_module
    (memory 1)
    (func (export "run") (result i32)
      (i32.eq (memory.grow (i32.const 100)) (i32.const -1))
      (i32.eqz)))
  (core instance $guest (instantiate $guest_module))
  (func $run (result (result)) (canon lift (core func $guest "run")))
  (instance $cli (export "run" (func $run)))
  (export "wasi:cli/run@0.2.12" (instance $cli)))"#,
    )
}

fn stream_imports(stdio: &str) -> String {
    format!(
        r#"
  (import "wasi:io/error@0.2.12" (instance $error
    (export "error" (type (sub resource)))))
  (alias export $error "error" (type $err))
  (import "wasi:io/streams@0.2.12" (instance $streams
    (alias outer 1 $err (type $err_outer))
    (export "error" (type $err_e (eq $err_outer)))
    (type $stream_error (variant (case "last-operation-failed" (own $err_e)) (case "closed")))
    (export "stream-error" (type $se (eq $stream_error)))
    (export "output-stream" (type $os (sub resource)))
    (export "[method]output-stream.blocking-write-and-flush"
      (func (param "self" (borrow $os)) (param "contents" (list u8)) (result (result (error $se)))))))
  (alias export $streams "output-stream" (type $os_outer))
  (import "wasi:cli/{stdio}@0.2.12" (instance $stdio
    (alias outer 1 $os_outer (type $os_for_stdio))
    (export "output-stream" (type $os_e (eq $os_for_stdio)))
    (export "get-{stdio}" (func (result (own $os_e))))))
  (core func $get (canon lower (func $stdio "get-{stdio}")))
"#
    )
}

/// Writes `hello from wasm\n` to stdout or stderr and succeeds.
pub fn prints_to(stdio: &str) -> Vec<u8> {
    parse(&format!(
        r#"(component
{streams}
{memory}
  (core func $write (canon lower (func $streams "[method]output-stream.blocking-write-and-flush") (memory $mem "memory")))
  (core instance $host (export "get" (func $get)) (export "write" (func $write)))
  (core module $guest_module
    (import "mem" "memory" (memory 2))
    (import "host" "get" (func $get (result i32)))
    (import "host" "write" (func $write (param i32 i32 i32 i32)))
    (func (export "run") (result i32)
      (call $write (call $get) (i32.const 8) (i32.const 16) (i32.const 512))
      (i32.load8_u (i32.const 512))))
{run}
)"#,
        streams = stream_imports(stdio),
        memory = MEMORY_MODULE,
        run = RUN_EXPORT,
    ))
}

/// Succeeds only when the guest was given at least one preopened directory.
pub fn succeeds_only_with_a_preopen() -> Vec<u8> {
    parse(&format!(
        r#"(component
  (import "wasi:filesystem/types@0.2.12" (instance $types
    (export "descriptor" (type (sub resource)))))
  (alias export $types "descriptor" (type $descriptor))
  (import "wasi:filesystem/preopens@0.2.12" (instance $preopens
    (alias outer 1 $descriptor (type $d))
    (export "descriptor" (type $d_e (eq $d)))
    (export "get-directories" (func (result (list (tuple (own $d_e) string)))))))
{memory}
  (core func $dirs (canon lower (func $preopens "get-directories") (memory $mem "memory") (realloc (func $mem "realloc"))))
  (core instance $host (export "dirs" (func $dirs)))
  (core module $guest_module
    (import "mem" "memory" (memory 2))
    (import "host" "dirs" (func $dirs (param i32)))
    (func (export "run") (result i32)
      (call $dirs (i32.const 512))
      (i32.eqz (i32.load (i32.const 516)))))
{run}
)"#,
        memory = MEMORY_MODULE,
        run = RUN_EXPORT,
    ))
}
