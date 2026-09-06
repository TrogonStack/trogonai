#![feature(rustc_private)]

extern crate rustc_ast;
extern crate rustc_hir;
extern crate rustc_lint;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;

mod acyclic_modules;
mod assertions_on_fixed_literals;
mod constant_outside_constants_module;
mod debug_remnants;
mod error_string_comparison;
mod error_type_naming;
mod fallible_new;
mod function_local_macro_rules;
mod function_local_use;
mod inline_module_block;
mod manual_error_impl;
mod redundant_module_path;
mod serde_json_macro;
mod serde_json_macro_allow_without_reason;
mod std_env_access;
mod telemetry_attribute_literal;
mod telemetry_key_value_literal;
mod telemetry_literal;
mod telemetry_metric_construction;
mod telemetry_metric_name_literal;
mod telemetry_span_name_literal;
mod test_context;
mod test_module_naming;
mod tracing_metadata;
mod unbounded_channel;
mod unstructured_log_fields;
mod weakened_write_precondition;

use rustc_hir::def_id::LocalDefId;
use rustc_hir::intravisit::FnKind;
use rustc_hir::{AmbigArg, Body, Expr, FnDecl, ImplItem, Item, LetStmt, Stmt, Ty};
use rustc_lint::{LateContext, LateLintPass, LintStore};
use rustc_span::Span;

dylint_linting::dylint_library!();

#[unsafe(no_mangle)]
pub fn register_lints(sess: &rustc_session::Session, lint_store: &mut LintStore) {
    dylint_linting::init_config(sess);
    lint_store.register_lints(&[
        ACYCLIC_MODULES,
        ASSERTIONS_ON_FIXED_LITERALS,
        CONSTANT_OUTSIDE_CONSTANTS_MODULE,
        DEBUG_REMNANTS,
        ERROR_STRING_COMPARISON,
        ERROR_TYPE_NAMING,
        FALLIBLE_NEW,
        FUNCTION_LOCAL_MACRO_RULES,
        FUNCTION_LOCAL_USE,
        INLINE_MODULE_BLOCK,
        MANUAL_ERROR_IMPL,
        REDUNDANT_MODULE_PATH,
        SERDE_JSON_MACRO,
        SERDE_JSON_MACRO_ALLOW_WITHOUT_REASON,
        STD_ENV_ACCESS,
        TELEMETRY_ATTRIBUTE_LITERAL,
        TELEMETRY_KEY_VALUE_LITERAL,
        TELEMETRY_METRIC_CONSTRUCTION,
        TELEMETRY_METRIC_NAME_LITERAL,
        TELEMETRY_SPAN_NAME_LITERAL,
        TEST_MODULE_NAMING,
        UNBOUNDED_CHANNEL,
        UNSTRUCTURED_LOG_FIELDS,
        WEAKENED_WRITE_PRECONDITION,
    ]);
    lint_store.register_late_pass(|_| Box::<TrogonLints>::default());
    lint_store.register_early_pass(|| Box::new(redundant_module_path::RedundantModulePath));
    lint_store
        .register_early_pass(|| Box::new(serde_json_macro_allow_without_reason::SerdeJsonMacroAllowWithoutReason));
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects always-true assertions in compile-time contexts whose condition
    /// merely checks a literal or a local constant initialized with a literal.
    ///
    /// ### Why is this bad?
    ///
    /// Restating an obvious property of a fixed value adds no independent
    /// invariant. Runtime validation, computed values, generated inputs, type
    /// properties, and relationships between distinct named constants remain
    /// meaningful and are outside this rule's scope.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// const VERSION_CURRENT: &str = "current";
    /// const _: () = assert!(!VERSION_CURRENT.is_empty());
    /// ```
    pub ASSERTIONS_ON_FIXED_LITERALS,
    Deny,
    "compile-time assertions must express more than an obvious property of a fixed literal",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects a cycle in the dependencies between sibling modules, at any
    /// depth of the module tree.
    ///
    /// ### Why is this bad?
    ///
    /// A cycle means neither module can be read, moved, or tested without the
    /// other, and it removes the one property that makes a module tree
    /// navigable: that dependencies point in a single direction. Cycles are
    /// never introduced deliberately. They accrue one reasonable edge at a
    /// time, until `payments` and `server` are a single unit spelled as two,
    /// and everything that depends on either depends on both.
    ///
    /// The rule is checked between siblings, so a cycle is reported against
    /// the module that owns both ends of it. Parent-child references are not
    /// dependencies in this sense and are never considered: a parent declares
    /// its children and re-exports their API, and a child reaches back through
    /// `super::`, which is how the module tree is built rather than how it is
    /// layered. Because sibling graphs aggregate every reference made anywhere
    /// in a child's subtree, a cycle distributed across grandchildren
    /// (`payments::checkout` to `server::auth`, `server::routes` back to
    /// `payments::billing`) is reported at the level where it closes.
    ///
    /// Only intra-crate references count, since Cargo already forbids cycles
    /// between crates. Macro-generated references are the macro author's, not
    /// the call site's, and are exempt. Test and benchmark code (`#[test]`
    /// functions, `#[cfg(test)]` items, Cargo `tests/`/`benches/` targets,
    /// `tests.rs`/`*_tests.rs` sources, and the test-support module family
    /// `tests`/`benches`/`test_support`/`mocks`/`fixtures`/`testkit`/`*_harness`)
    /// reaches across the tree by design and says nothing about how production
    /// code is layered, so it is exempt too.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// // in payments/checkout.rs
    /// use crate::server::auth::verify;
    ///
    /// // in server/auth.rs
    /// use crate::payments::billing::Invoice;
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// // in session.rs, which neither `payments` nor `server` owns
    /// pub struct Session { ... }
    ///
    /// // in payments/checkout.rs
    /// use crate::session::Session;
    ///
    /// // in server/auth.rs
    /// use crate::session::Session;
    /// ```
    pub ACYCLIC_MODULES,
    Deny,
    "module dependencies must flow in one direction, not in a cycle",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects a module-level (or crate-root) `const` declared in any file other
    /// than a `constants.rs`.
    ///
    /// ### Why is this bad?
    ///
    /// Constants live in a `constants` module, so a module's tunable values
    /// (ports, buffer sizes, header names, bounds) are discoverable in one place
    /// instead of scattered across the modules that happen to use them. A crate
    /// may have more than one: a crate-root `constants.rs` plus a nested
    /// `constants.rs` per submodule group is the intended shape (for example
    /// `src/constants.rs` alongside `src/source/<name>/constants.rs`). Spreading
    /// `const` items through the rest of the tree hides that configuration
    /// surface and invites the same value to be redefined in two files. Constants inside a function body are local implementation
    /// details and are left alone; associated consts (`impl`/`trait`) are not
    /// free items and are never considered. `static` items are a different
    /// construct and are out of scope. Test and benchmark sources (`tests.rs`,
    /// `*_tests.rs`, anything under a `tests/`/`benches/` directory, and inline
    /// `tests`/`benches` modules and the not-for-prod test-support module family
    /// `test_support`/`mocks`/`fixtures`/`testkit`/`*_harness`) carry fixtures and
    /// per-case values rather than crate configuration, so they are exempt. Generated files (those carrying
    /// an `@generated` marker near the top) are exempt too, since their contents
    /// are dictated by codegen and cannot be hand-edited.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// // in transport.rs
    /// const MAX_INSPECTED_BODY: usize = 1024 * 1024;
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// // in constants.rs
    /// pub const MAX_INSPECTED_BODY: usize = 1024 * 1024;
    ///
    /// // in transport.rs
    /// use constants::MAX_INSPECTED_BODY;
    /// ```
    pub CONSTANT_OUTSIDE_CONSTANTS_MODULE,
    Deny,
    "declare module-level constants in a `constants` module, not elsewhere",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects `println!`, `print!`, `eprintln!`, and `dbg!` in code that ships.
    ///
    /// ### Why is this bad?
    ///
    /// These macros write to the process's own stdout and stderr, a channel
    /// nothing operates on. The line carries no level, so it cannot be
    /// filtered; no fields, so it cannot be queried; no span context, so it
    /// cannot be tied to the request that produced it; and it bypasses the
    /// subscriber entirely, so it never reaches the backend the service
    /// exports its logs to. A `dbg!` additionally prints the expression's
    /// source text and keeps evaluating it in production, at a cost nobody
    /// chose.
    ///
    /// The usual reason one of these is here at all is that it was typed to
    /// answer a question during development and never removed. `tracing` is
    /// how this repository records what a service is doing, and the same line
    /// promoted to an event gains a level, named fields, and the span it
    /// happened in.
    ///
    /// `eprint!` is not reported: a write to stderr with no newline is how a
    /// terminal program draws a prompt or a progress line, which is output
    /// rather than a leftover debugging statement.
    ///
    /// Test sources are exempt, where printing is how a failing case explains
    /// itself to whoever reads the output. A write that is genuinely the
    /// program's own output (a CLI printing its result, a process reporting a
    /// fatal error before a subscriber exists) is not exempt by position,
    /// because nothing distinguishes it from a leftover line except intent; it
    /// says so at the site with
    /// `#[cfg_attr(dylint_lib = "trogon_lints", allow(debug_remnants, reason = "..."))]`.
    ///
    /// This rule is ported from the `debug_remnants` lint in
    /// <https://github.com/li-kai/rust-lints>; the credit for it is theirs.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// println!("request: {:?}", request);
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// tracing::info!(?request, "request received");
    /// ```
    pub DEBUG_REMNANTS,
    Deny,
    "record diagnostics as `tracing` events, not as writes to the process's stdout or stderr",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects equality checks and string probes that are driven by
    /// `std::error::Error` display text.
    ///
    /// ### Why is this bad?
    ///
    /// Display text is presentation, not a semantic contract. If behavior
    /// depends on the text returned by `Error::to_string`, changing a human
    /// message can change retry policy, authorization decisions, status
    /// mapping, or protocol behavior.
    ///
    /// ### Example
    ///
    /// ```rust
    /// # fn classify(error: anyhow::Error) -> bool {
    /// error.to_string().contains("not found")
    /// # }
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust
    /// # enum DomainError { NotFound }
    /// # fn classify(error: DomainError) -> bool {
    /// matches!(error, DomainError::NotFound)
    /// # }
    /// ```
    pub ERROR_STRING_COMPARISON,
    Deny,
    "error display strings must not drive behavior",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects a type that implements `std::error::Error` (whether hand-written
    /// or derived with `thiserror`) whose name does not end in `Error`.
    ///
    /// ### Why is this bad?
    ///
    /// The `Error` suffix is the signal, readable at any call site, that a type
    /// is a failure value. Without it a reader has to find the type's definition
    /// to learn whether `Err(EmptySecret)` returns an error or some marker value,
    /// and error types blend in with ordinary structs and enums. Requiring the
    /// suffix on every type that implements `Error` makes the role legible from
    /// the name alone. A type named exactly `Error` (the crate- or module-root
    /// error enum) already satisfies the rule.
    ///
    /// ### Example
    ///
    /// ```rust
    /// #[derive(Debug, thiserror::Error)]
    /// #[error("secret must not be empty")]
    /// pub struct EmptySecret;
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust
    /// #[derive(Debug, thiserror::Error)]
    /// #[error("secret must not be empty")]
    /// pub struct EmptySecretError;
    /// ```
    pub ERROR_TYPE_NAMING,
    Deny,
    "name types that implement `std::error::Error` with an `Error` suffix",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects a constructor named `new` (or a `new_*` variant) whose body can
    /// panic: `.unwrap()` or `.expect(...)` on a `Result` or an `Option`, a
    /// `panic!`, or an `unreachable!`.
    ///
    /// ### Why is this bad?
    ///
    /// `new` is the name Rust reserves for construction that cannot fail, and a
    /// caller reads it that way: there is no `Err` to match, nothing for `?` to
    /// carry, and no way to recover short of `catch_unwind`. A panic reached
    /// through that name takes the whole process down for a failure the caller
    /// was never given the chance to handle, and a library consumer cannot wrap
    /// the call in error handling of their own. Returning `Result<Self, _>`
    /// (renaming to `try_new` when an infallible constructor stays beside it),
    /// or moving the fallible work out to the caller, puts the failure back in
    /// the signature where it can be seen.
    ///
    /// `todo!` and `unimplemented!` are left to rustc's own lints for
    /// unfinished code. A constructor that already returns `Result` or `Option`
    /// has said its construction can fail and is left alone, as is one in a
    /// trait impl, where the implementor owns neither the name nor the return
    /// type. Test and benchmark sources are exempt, since a panic there fails
    /// the test that caused it rather than surprising a caller.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// impl Client {
    ///     pub fn new(endpoint: &str) -> Self {
    ///         Self {
    ///             endpoint: Url::parse(endpoint).expect("invalid endpoint"),
    ///         }
    ///     }
    /// }
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// impl Client {
    ///     pub fn new(endpoint: &str) -> Result<Self, ParseError> {
    ///         Ok(Self {
    ///             endpoint: Url::parse(endpoint)?,
    ///         })
    ///     }
    /// }
    /// ```
    pub FALLIBLE_NEW,
    Deny,
    "a constructor named `new` must not panic; return `Result` or rename it `try_new`",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects inline modules declared with a body block (`mod foo { ... }`)
    /// instead of being backed by their own file (`mod foo;`).
    ///
    /// ### Why is this bad?
    ///
    /// Module bodies belong in files. Inline blocks bury structure inside an
    /// unrelated file, grow without bound, and make navigation depend on
    /// scrolling rather than the file tree. A file-per-module layout keeps the
    /// physical structure and the module structure in sync. A child module in
    /// its own file still reaches the parent module's private items, so the
    /// usual reason to inline `#[cfg(test)] mod tests { ... }` does not apply.
    /// Generated files (those carrying an `@generated` marker near the top) are
    /// exempt, since their module layout is dictated by codegen.
    ///
    /// ### Example
    ///
    /// ```rust
    /// mod twin {
    ///     pub fn run() {}
    /// }
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust
    /// // in twin.rs
    /// pub fn run() {}
    /// ```
    ///
    /// ```rust
    /// // in the parent
    /// mod twin;
    /// ```
    pub INLINE_MODULE_BLOCK,
    Deny,
    "declare modules in their own file with `mod foo;`, not inline `mod foo { ... }`",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects hand-written `impl std::error::Error` blocks.
    ///
    /// ### Why is this bad?
    ///
    /// Manual `Error` (and the `Display` it depends on) implementations are
    /// boilerplate that drifts: a new variant can be forgotten in `Display`
    /// or `source`, and the wiring is easy to get subtly wrong. Deriving the
    /// error keeps the message and source chain declarative next to each
    /// variant.
    ///
    /// ### Example
    ///
    /// ```rust
    /// # use std::fmt;
    /// # enum MyError { Io(std::io::Error) }
    /// # impl fmt::Display for MyError {
    /// #     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("io error") }
    /// # }
    /// impl std::error::Error for MyError {}
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust
    /// #[derive(Debug, thiserror::Error)]
    /// enum MyError {
    ///     #[error("io error")]
    ///     Io(#[from] std::io::Error),
    /// }
    /// ```
    pub MANUAL_ERROR_IMPL,
    Deny,
    "implement `std::error::Error` with the thiserror derive, not by hand",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects a `#[path = "..."]` attribute on an external module declaration
    /// (`mod foo;`) when it points at the very file Rust would resolve to on its
    /// own.
    ///
    /// ### Why is this bad?
    ///
    /// For a file-backed module `bar.rs`, `mod foo;` already resolves to
    /// `bar/foo.rs` (or `bar/foo/mod.rs`); for a directory owner (`mod.rs`,
    /// `lib.rs`, `main.rs`) it resolves to the sibling `foo.rs`. Spelling that
    /// default out with `#[path]` is noise that must be hand-maintained and kept
    /// in sync with the file tree. The common case is `#[cfg(test)] mod tests;`
    /// whose `tests.rs` already sits in the conventional location. A `#[path]`
    /// pointing anywhere other than the default is load-bearing and is left
    /// alone.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// // in bar.rs, next to bar/tests.rs
    /// #[cfg(test)]
    /// #[path = "bar/tests.rs"]
    /// mod tests;
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// // in bar.rs, next to bar/tests.rs
    /// #[cfg(test)]
    /// mod tests;
    /// ```
    pub REDUNDANT_MODULE_PATH,
    Deny,
    "drop `#[path]` when `mod foo;` already resolves to the same file",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects a `serde_json::json!` invocation in production code.
    ///
    /// ### Why is this bad?
    ///
    /// A `json!` literal is an anonymous payload shape. The keys are strings the
    /// compiler never checks, the value types are whatever the literal happens to
    /// hold, and the same wire shape gets respelled at every site that builds
    /// it, so a renamed field is found by whoever reads the failing payload
    /// rather than by `cargo check`. Modelling the payload as a type with
    /// `#[derive(Serialize)]` gives the shape a name, a single definition, and a
    /// schema that the consumer side can deserialize into, and
    /// `serde_json::to_value` still produces the `Value` the API wanted. Tests
    /// are exempt: a fixture's whole job is to spell a payload out literally,
    /// including the malformed ones no type can express. A genuinely dynamic
    /// shape (pass-through of a foreign document, a protocol whose fields are
    /// only known at runtime) is a real exception, so the lint is suppressible at
    /// the site with a stated reason. Generated files (those carrying an
    /// `@generated` marker near the top) are exempt too, since their contents are
    /// dictated by codegen.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// let body = serde_json::json!({
    ///     "error": { "code": code, "message": message },
    /// });
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// #[derive(serde::Serialize)]
    /// struct ErrorBody {
    ///     error: ErrorDetail,
    /// }
    ///
    /// #[derive(serde::Serialize)]
    /// struct ErrorDetail {
    ///     code: i32,
    ///     message: String,
    /// }
    ///
    /// let body = serde_json::to_value(ErrorBody {
    ///     error: ErrorDetail { code, message },
    /// })?;
    /// ```
    pub SERDE_JSON_MACRO,
    Deny,
    "build JSON payloads from a `Serialize` type, not an ad-hoc `serde_json::json!` literal",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects an `allow(serde_json_macro)` (or `expect(serde_json_macro)`)
    /// attribute that carries no `reason = "..."`.
    ///
    /// ### Why is this bad?
    ///
    /// `serde_json_macro` is suppressible because a genuinely dynamic payload
    /// is a real exception, not because the rule is optional. Rust accepts a
    /// bare `allow`, so without this check the escape hatch costs one line and
    /// records nothing: the next reader cannot tell an argued exception from a
    /// silenced diagnostic, and cannot tell when the exception stopped being
    /// true. Requiring the reason keeps the justification next to the code it
    /// justifies, where review sees it.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// #[cfg_attr(dylint_lib = "trogon_lints", allow(serde_json_macro))]
    /// fn passthrough(document: &RawValue) -> Value { ... }
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// #[cfg_attr(
    ///     dylint_lib = "trogon_lints",
    ///     allow(
    ///         serde_json_macro,
    ///         reason = "the upstream document is forwarded verbatim and has no fixed schema"
    ///     )
    /// )]
    /// fn passthrough(document: &RawValue) -> Value { ... }
    /// ```
    pub SERDE_JSON_MACRO_ALLOW_WITHOUT_REASON,
    Deny,
    "state the technical reason when suppressing `serde_json_macro`",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects a direct call to a `std::env` environment-variable reader
    /// (`var`, `var_os`, `vars`, `vars_os`) outside the `trogon-std` crate.
    ///
    /// ### Why is this bad?
    ///
    /// Reading process environment variables at the call site couples logic to
    /// ambient global state: the value cannot be supplied in a test without
    /// mutating the real environment, which is process-global and races across
    /// parallel tests. `trogon-std` models the read as the `ReadEnv` trait, with
    /// `SystemEnv` in production and `InMemoryEnv` in tests, so a unit can take
    /// `&impl ReadEnv` and be driven deterministically. Calling `std::env`
    /// directly forks that contract and reintroduces the untestable global. The
    /// `SystemEnv` implementation inside `trogon-std` is the one place allowed to
    /// reach `std::env`, so the lint is silent there.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// let url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".into());
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// use trogon_std::env::ReadEnv;
    ///
    /// fn nats_url(env: &impl ReadEnv) -> String {
    ///     env.var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".into())
    /// }
    /// ```
    pub STD_ENV_ACCESS,
    Deny,
    "read environment variables through an injected `trogon_std::env::ReadEnv`, not `std::env` directly",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects `macro_rules!` definitions declared inside a function body.
    ///
    /// ### Why is this bad?
    ///
    /// Function-local macros lean on definition-site hygiene to silently
    /// capture surrounding local variables, which hides the macro's real
    /// inputs and pins the definition inside an unrelated function. At module
    /// scope every input must be passed explicitly, the macro is visible in
    /// the file structure, and it can be reused and reviewed on its own.
    ///
    /// ### Example
    ///
    /// ```rust
    /// fn build() {
    ///     let base = 1;
    ///     macro_rules! add {
    ///         ($x:expr) => { base + $x };
    ///     }
    ///     let _ = add!(2);
    /// }
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust
    /// macro_rules! add {
    ///     ($base:expr, $x:expr) => { $base + $x };
    /// }
    ///
    /// fn build() {
    ///     let base = 1;
    ///     let _ = add!(base, 2);
    /// }
    /// ```
    pub FUNCTION_LOCAL_MACRO_RULES,
    Deny,
    "macro_rules! definitions must live at module scope",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects `use` declarations placed inside a function body (or any block)
    /// instead of at module level.
    ///
    /// ### Why is this bad?
    ///
    /// `use` is pure name resolution: a function-local import is never required,
    /// since every name it brings in is equally reachable by its full path or by
    /// a module-level `use` (with `as` for collisions). Hiding imports inside
    /// function bodies scatters a module's dependency surface across its
    /// functions, so what a file depends on can no longer be read from the top
    /// of the file. Keep imports at module level where they are discoverable.
    /// Macro-generated imports come from expansion and are exempt.
    ///
    /// ### Example
    ///
    /// ```rust
    /// fn render(value: u8) -> String {
    ///     use std::fmt::Write;
    ///     let mut out = String::new();
    ///     write!(out, "{value}").unwrap();
    ///     out
    /// }
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust
    /// use std::fmt::Write;
    ///
    /// fn render(value: u8) -> String {
    ///     let mut out = String::new();
    ///     write!(out, "{value}").unwrap();
    ///     out
    /// }
    /// ```
    pub FUNCTION_LOCAL_USE,
    Deny,
    "declare `use` imports at module level, not inside a function body",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects modules that contain `#[test]` (or `#[tokio::test]`) functions
    /// but are not named `tests` or `*_tests`.
    ///
    /// ### Why is this bad?
    ///
    /// Test suites live in their own file-backed module (see
    /// `inline_module_block`), and that module's name becomes its file name. A
    /// fixed naming rule keeps test files discoverable and uniform: `tests` for
    /// the sole test module of a parent, and `*_tests` for siblings that split
    /// tests by concern or `cfg` gate (`parse_tests`, `cov_stub_tests`). Modules
    /// that only hold test *support* (mocks, fixtures, testkit) carry no `#[test]`
    /// functions and are left alone.
    ///
    /// ### Example
    ///
    /// ```rust
    /// // in helpers.rs
    /// #[test]
    /// fn it_works() {}
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust
    /// // in tests.rs (or parse_tests.rs for a sibling module)
    /// #[test]
    /// fn it_works() {}
    /// ```
    pub TEST_MODULE_NAMING,
    Deny,
    "name a module of tests `tests` or `*_tests`",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects a string literal passed as the field-name argument to
    /// `tracing::Span::record`.
    ///
    /// ### Why is this bad?
    ///
    /// Telemetry identifiers are a contract shared with dashboards, alerts, and
    /// trace correlation. The semantic-convention registry is the single source
    /// of truth for that contract, and `trogon-semconv` generates a constant for
    /// every attribute key. Spelling a key inline as `"session_id"` forks that
    /// contract: a registry rename no longer reaches the call site, and a typo
    /// silently records onto a field nobody queries. Recording through the
    /// generated constant keeps every emitted key tied to the registry.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// tracing::Span::current().record("session_id", id.as_str());
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// use trogon_semconv::attribute::SESSION_ID;
    ///
    /// tracing::Span::current().record(SESSION_ID, id.as_str());
    /// ```
    pub TELEMETRY_ATTRIBUTE_LITERAL,
    Deny,
    "record telemetry fields with a generated `trogon_semconv` constant, not an inline string literal",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects a string literal passed as the key argument to
    /// `opentelemetry::KeyValue::new`.
    ///
    /// ### Why is this bad?
    ///
    /// A `KeyValue` key is a telemetry attribute identifier, the same contract
    /// that `tracing::Span::record` carries (see `telemetry_attribute_literal`).
    /// The semantic-convention registry is the single source of truth for that
    /// contract, and `trogon-semconv` generates a constant for every attribute
    /// key. Spelling the key inline as `"messaging.system"` forks the contract:
    /// a registry rename no longer reaches the call site, and a typo silently
    /// emits onto an attribute nobody queries.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// KeyValue::new("messaging.system", "nats");
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// use trogon_semconv::attribute::MESSAGING_SYSTEM;
    ///
    /// KeyValue::new(MESSAGING_SYSTEM, "nats");
    /// ```
    pub TELEMETRY_KEY_VALUE_LITERAL,
    Deny,
    "build `KeyValue` keys from a generated `trogon_semconv` constant, not an inline string literal",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects a string literal passed as the instrument-name argument to an
    /// `opentelemetry` `Meter` builder (`u64_counter`, `f64_histogram`, the
    /// gauge and up-down-counter variants, and their observable forms).
    ///
    /// ### Why is this bad?
    ///
    /// A metric name is a telemetry contract shared with dashboards and alerts.
    /// The semantic-convention registry is the single source of truth, and
    /// `trogon-semconv` generates a constant for every metric. Spelling the name
    /// inline as `"acp.requests"` forks that contract: a registry rename no
    /// longer reaches the call site, and a typo silently creates a second,
    /// unqueried instrument.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// meter.u64_counter("acp.requests");
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// use trogon_semconv::metric::ACP_REQUESTS;
    ///
    /// meter.u64_counter(ACP_REQUESTS);
    /// ```
    pub TELEMETRY_METRIC_NAME_LITERAL,
    Deny,
    "name metric instruments with a generated `trogon_semconv` constant, not an inline string literal",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects an `opentelemetry` `Meter` instrument builder (`u64_counter`,
    /// `f64_histogram`, the gauge and up-down-counter variants, and their
    /// observable forms) opened anywhere outside the `trogon-semconv` crate.
    ///
    /// ### Why is this bad?
    ///
    /// A metric is a contract of name, description, and unit, all defined once in
    /// the semantic-convention registry. `trogon-semconv` generates a `build_*`
    /// constructor per metric that bakes those three together. Opening the
    /// builder inline at a call site restates the description and unit by hand,
    /// where they drift from the registry (a renamed metric, a reworded
    /// description, or a dropped unit goes unnoticed). Routing every instrument
    /// through its generated constructor keeps the whole contract single-sourced.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// let requests = meter
    ///     .u64_counter(metric::ACP_REQUESTS)
    ///     .with_description("Total number of ACP requests")
    ///     .build();
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// let requests = trogon_semconv::metric::build_acp_requests(&meter);
    /// ```
    pub TELEMETRY_METRIC_CONSTRUCTION,
    Deny,
    "construct metric instruments through a generated `trogon_semconv::metric::build_*` constructor",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects a string literal used as the span name in a `tracing`
    /// span-construction macro (`info_span!`, `span!`, and the other level
    /// variants) or in `#[instrument(name = "...")]`.
    ///
    /// ### Why is this bad?
    ///
    /// A span name is a telemetry contract shared with trace search and
    /// dashboards. The semantic-convention registry is the single source of
    /// truth, and `trogon-semconv` generates a constant for every span name.
    /// Spelling the name inline as `"http.server.request"` forks that contract:
    /// a registry rename no longer reaches the call site, and a typo silently
    /// produces a span nobody correlates on. Field names inside the macro are
    /// left alone; `tracing` requires those to be bare identifiers, so they
    /// cannot reference a constant.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// tracing::info_span!("http.server.request", method = %method);
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// use trogon_semconv::span::HTTP_SERVER_REQUEST;
    ///
    /// tracing::info_span!(HTTP_SERVER_REQUEST, method = %method);
    /// ```
    pub TELEMETRY_SPAN_NAME_LITERAL,
    Deny,
    "name spans with a generated `trogon_semconv` constant, not an inline string literal",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects the creation of a channel whose queue has no capacity:
    /// `std::sync::mpsc::channel`, `tokio::sync::mpsc::unbounded_channel`,
    /// `futures::channel::mpsc::unbounded`, `flume::unbounded`,
    /// `crossbeam::channel::unbounded`, and `async_channel::unbounded`.
    ///
    /// ### Why is this bad?
    ///
    /// An unbounded queue turns a throughput mismatch into a memory leak. A
    /// producer that outruns its consumer never learns that it is ahead: the
    /// send always succeeds, the queue grows for as long as the imbalance
    /// lasts, and the first symptom is the process being killed for its
    /// resident size. The imbalance does not have to be permanent to do this,
    /// because a burst arriving faster than the consumer drains is enough.
    ///
    /// A capacity is what makes the mismatch observable and survivable. When
    /// the queue is full the sender waits (or is told the queue is full), so
    /// backpressure reaches the producer, and through it whatever the producer
    /// is reading from: a socket stops being drained, a JetStream consumer
    /// stops acking, an HTTP handler stops accepting. The bound also names the
    /// worst-case memory the queue can hold, which an unbounded channel leaves
    /// as a property of the workload.
    ///
    /// Test sources are exempt: a test drives both ends of its channel, so the
    /// producer cannot outrun the consumer in a way that outlives the test.
    ///
    /// This rule is ported from the `unbounded_channel` lint in
    /// <https://github.com/li-kai/rust-lints>; the credit for it is theirs.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// let (tx, rx) = tokio::sync::mpsc::channel(EVENT_QUEUE_CAPACITY);
    /// ```
    pub UNBOUNDED_CHANNEL,
    Deny,
    "give a channel an explicit capacity, so a slow consumer applies backpressure instead of consuming memory",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects a `tracing` event macro (`info!`, `warn!`, `error!`, `debug!`,
    /// `trace!`, `event!`) whose captured values are all interpolated into the
    /// message through format placeholders, with no structured field among
    /// them.
    ///
    /// ### Why is this bad?
    ///
    /// A field is queryable and a message is not. `tracing` records fields as
    /// typed key-value pairs that a subscriber can filter, index, and forward
    /// to a log backend as columns; a value baked into the message text is
    /// reachable only by whoever writes the right regular expression against
    /// the rendered string. Interpolating the value also drops its name, so
    /// the same quantity is `session {}` at one site and `for session {}` at
    /// the next, and nothing ties them together. Naming the value as a field
    /// costs the same number of tokens and keeps the message the constant part
    /// a reader scans for. A callsite that already carries at least one field
    /// is left alone: mixing a field with a formatted message is a deliberate
    /// middle ground, not an oversight. A message with no placeholders
    /// captures nothing and has nothing to structure. Test sources
    /// (`tests.rs`, `*_tests.rs`) are exempt.
    ///
    /// This rule is ported from the `unstructured_log_fields` lint in
    /// <https://github.com/li-kai/rust-lints>; the credit for it is theirs.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// tracing::info!("user {} hit {}", user_id, path);
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// tracing::info!(user_id, path, "user hit endpoint");
    /// ```
    pub UNSTRUCTURED_LOG_FIELDS,
    Deny,
    "record log values as `tracing` fields, not as format arguments in the message",
}

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Detects a decider whose `WRITE_PRECONDITION` associated const is
    /// `WritePrecondition::Any`, unless the declaration carries an `allow` that
    /// states a reason.
    ///
    /// ### Why is this bad?
    ///
    /// `Any` is the one variant that appends without checking anything first,
    /// so the decision a command made from the state it read is written even if
    /// another writer changed that state in between. That is occasionally
    /// correct, for a fact that commutes with every other fact on the stream,
    /// and it is silently wrong for anything that guards an invariant. The
    /// difference lives entirely in the *why*, which is invisible at a call
    /// site that reads `WritePrecondition::Any` and nothing else.
    ///
    /// The const already makes every decider's choice visible. This makes the
    /// one dangerous choice argued: an `allow` with a `reason` records the
    /// commutativity claim next to the code that depends on it, where a
    /// reviewer can disagree with it. A bare `allow` is reported, because
    /// silencing the question is not answering it.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// impl Decider for RenameSession {
    ///     const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::Any;
    /// }
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// impl Decider for RenameSession {
    ///     const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamExists;
    /// }
    /// ```
    ///
    /// or, where the event genuinely commutes:
    ///
    /// ```rust,ignore
    /// impl Decider for RenameSession {
    ///     #[cfg_attr(
    ///         dylint_lib = "trogon_lints",
    ///         allow(
    ///             weakened_write_precondition,
    ///             reason = "a rename is a last-writer-wins fact that guards no invariant"
    ///         )
    ///     )]
    ///     const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::Any;
    /// }
    /// ```
    pub WEAKENED_WRITE_PRECONDITION,
    Deny,
    "argue an unconditional `WritePrecondition::Any` append, or name the invariant it depends on",
}

#[derive(Default)]
struct TrogonLints {
    acyclic_modules: acyclic_modules::AcyclicModules,
    debug_remnants: debug_remnants::DebugRemnants,
    error_string_comparison: error_string_comparison::ErrorStringComparison,
    function_local_use: function_local_use::FunctionLocalUse,
    serde_json_macro: serde_json_macro::SerdeJsonMacro,
    std_env_access: std_env_access::StdEnvAccess,
    telemetry_attribute_literal: telemetry_attribute_literal::TelemetryAttributeLiteral,
    telemetry_key_value_literal: telemetry_key_value_literal::TelemetryKeyValueLiteral,
    telemetry_metric_construction: telemetry_metric_construction::TelemetryMetricConstruction,
    telemetry_metric_name_literal: telemetry_metric_name_literal::TelemetryMetricNameLiteral,
    telemetry_span_name_literal: telemetry_span_name_literal::TelemetrySpanNameLiteral,
    unstructured_log_fields: unstructured_log_fields::UnstructuredLogFields,
}

impl<'tcx> LateLintPass<'tcx> for TrogonLints {
    fn check_local(&mut self, cx: &LateContext<'tcx>, local: &'tcx LetStmt<'tcx>) {
        self.error_string_comparison.check_local(cx, local);
    }

    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        assertions_on_fixed_literals::check_expr(cx, expr);
        unbounded_channel::check_expr(cx, expr);
        self.acyclic_modules.check_expr(cx, expr);
        self.debug_remnants.check_expr(cx, expr);
        self.error_string_comparison.check_expr(cx, expr);
        self.serde_json_macro.check_expr(cx, expr);
        self.std_env_access.check_expr(cx, expr);
        self.telemetry_attribute_literal.check_expr(cx, expr);
        self.telemetry_key_value_literal.check_expr(cx, expr);
        self.telemetry_metric_construction.check_expr(cx, expr);
        self.telemetry_metric_name_literal.check_expr(cx, expr);
        self.telemetry_span_name_literal.check_expr(cx, expr);
        self.unstructured_log_fields.check_expr(cx, expr);
    }

    fn check_fn(
        &mut self,
        cx: &LateContext<'tcx>,
        kind: FnKind<'tcx>,
        _: &'tcx FnDecl<'tcx>,
        body: &'tcx Body<'tcx>,
        span: Span,
        def_id: LocalDefId,
    ) {
        fallible_new::check_fn(cx, kind, body, span, def_id);
    }

    fn check_stmt(&mut self, cx: &LateContext<'tcx>, stmt: &'tcx Stmt<'tcx>) {
        function_local_macro_rules::check_stmt(cx, stmt);
        self.function_local_use.check_stmt(cx, stmt);
    }

    fn check_ty(&mut self, cx: &LateContext<'tcx>, ty: &'tcx Ty<'tcx, AmbigArg>) {
        self.acyclic_modules.check_ty(cx, ty);
    }

    fn check_item(&mut self, cx: &LateContext<'tcx>, item: &'tcx Item<'tcx>) {
        self.acyclic_modules.check_item(cx, item);
        constant_outside_constants_module::check_item(cx, item);
        error_type_naming::check_item(cx, item);
        inline_module_block::check_item(cx, item);
        manual_error_impl::check_item(cx, item);
        test_module_naming::check_item(cx, item);
    }

    fn check_impl_item(&mut self, cx: &LateContext<'tcx>, impl_item: &'tcx ImplItem<'tcx>) {
        weakened_write_precondition::check_impl_item(cx, impl_item);
    }

    fn check_crate_post(&mut self, cx: &LateContext<'tcx>) {
        self.acyclic_modules.check_crate_post(cx);
        self.unstructured_log_fields.check_crate_post(cx);
    }
}

rustc_session::impl_lint_pass!(TrogonLints => [
    ACYCLIC_MODULES,
    ASSERTIONS_ON_FIXED_LITERALS,
    CONSTANT_OUTSIDE_CONSTANTS_MODULE,
    DEBUG_REMNANTS,
    ERROR_STRING_COMPARISON,
    ERROR_TYPE_NAMING,
    FALLIBLE_NEW,
    FUNCTION_LOCAL_MACRO_RULES,
    FUNCTION_LOCAL_USE,
    INLINE_MODULE_BLOCK,
    MANUAL_ERROR_IMPL,
    SERDE_JSON_MACRO,
    STD_ENV_ACCESS,
    TELEMETRY_ATTRIBUTE_LITERAL,
    TELEMETRY_KEY_VALUE_LITERAL,
    TELEMETRY_METRIC_CONSTRUCTION,
    TELEMETRY_METRIC_NAME_LITERAL,
    TELEMETRY_SPAN_NAME_LITERAL,
    TEST_MODULE_NAMING,
    UNBOUNDED_CHANNEL,
    UNSTRUCTURED_LOG_FIELDS,
    WEAKENED_WRITE_PRECONDITION,
]);

rustc_session::impl_lint_pass!(redundant_module_path::RedundantModulePath => [REDUNDANT_MODULE_PATH]);

rustc_session::impl_lint_pass!(
    serde_json_macro_allow_without_reason::SerdeJsonMacroAllowWithoutReason
        => [SERDE_JSON_MACRO_ALLOW_WITHOUT_REASON]
);

#[test]
fn ui() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui");
}

// The telemetry lints gate on real `tracing` / `opentelemetry` types, so their
// fixtures live as example targets where they can depend on those crates; the
// dependency-free `ui` directory cannot. All examples run under a single test:
// the compiletest harness mutates process-global state, so concurrent example
// runs race.
#[test]
fn ui_examples() {
    dylint_testing::ui_test_examples(env!("CARGO_PKG_NAME"));
}
