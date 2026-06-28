#![feature(rustc_private)]

extern crate rustc_ast;
extern crate rustc_hir;
extern crate rustc_lint;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;

mod error_string_comparison;
mod function_local_use;
mod inline_module_block;
mod manual_error_impl;
mod redundant_module_path;
mod std_env_access;
mod telemetry_attribute_literal;
mod telemetry_key_value_literal;
mod telemetry_literal;
mod telemetry_metric_construction;
mod telemetry_metric_name_literal;
mod telemetry_span_name_literal;
mod test_module_naming;

use rustc_hir::{Expr, Item, LetStmt, Stmt};
use rustc_lint::{LateContext, LateLintPass, LintStore};

dylint_linting::dylint_library!();

#[unsafe(no_mangle)]
pub fn register_lints(sess: &rustc_session::Session, lint_store: &mut LintStore) {
    dylint_linting::init_config(sess);
    lint_store.register_lints(&[
        ERROR_STRING_COMPARISON,
        FUNCTION_LOCAL_USE,
        INLINE_MODULE_BLOCK,
        MANUAL_ERROR_IMPL,
        REDUNDANT_MODULE_PATH,
        STD_ENV_ACCESS,
        TELEMETRY_ATTRIBUTE_LITERAL,
        TELEMETRY_KEY_VALUE_LITERAL,
        TELEMETRY_METRIC_CONSTRUCTION,
        TELEMETRY_METRIC_NAME_LITERAL,
        TELEMETRY_SPAN_NAME_LITERAL,
        TEST_MODULE_NAMING,
    ]);
    lint_store.register_late_pass(|_| Box::<TrogonLints>::default());
    lint_store.register_early_pass(|| Box::new(redundant_module_path::RedundantModulePath));
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
    /// Detects a direct call to `std::env::var` outside the `trogon-std` crate.
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

#[derive(Default)]
struct TrogonLints {
    error_string_comparison: error_string_comparison::ErrorStringComparison,
    function_local_use: function_local_use::FunctionLocalUse,
    std_env_access: std_env_access::StdEnvAccess,
    telemetry_attribute_literal: telemetry_attribute_literal::TelemetryAttributeLiteral,
    telemetry_key_value_literal: telemetry_key_value_literal::TelemetryKeyValueLiteral,
    telemetry_metric_construction: telemetry_metric_construction::TelemetryMetricConstruction,
    telemetry_metric_name_literal: telemetry_metric_name_literal::TelemetryMetricNameLiteral,
    telemetry_span_name_literal: telemetry_span_name_literal::TelemetrySpanNameLiteral,
}

impl<'tcx> LateLintPass<'tcx> for TrogonLints {
    fn check_local(&mut self, cx: &LateContext<'tcx>, local: &'tcx LetStmt<'tcx>) {
        self.error_string_comparison.check_local(cx, local);
    }

    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        self.error_string_comparison.check_expr(cx, expr);
        self.std_env_access.check_expr(cx, expr);
        self.telemetry_attribute_literal.check_expr(cx, expr);
        self.telemetry_key_value_literal.check_expr(cx, expr);
        self.telemetry_metric_construction.check_expr(cx, expr);
        self.telemetry_metric_name_literal.check_expr(cx, expr);
        self.telemetry_span_name_literal.check_expr(cx, expr);
    }

    fn check_stmt(&mut self, cx: &LateContext<'tcx>, stmt: &'tcx Stmt<'tcx>) {
        self.function_local_use.check_stmt(cx, stmt);
    }

    fn check_item(&mut self, cx: &LateContext<'tcx>, item: &'tcx Item<'tcx>) {
        inline_module_block::check_item(cx, item);
        manual_error_impl::check_item(cx, item);
        test_module_naming::check_item(cx, item);
    }
}

rustc_session::impl_lint_pass!(TrogonLints => [
    ERROR_STRING_COMPARISON,
    FUNCTION_LOCAL_USE,
    INLINE_MODULE_BLOCK,
    MANUAL_ERROR_IMPL,
    STD_ENV_ACCESS,
    TELEMETRY_ATTRIBUTE_LITERAL,
    TELEMETRY_KEY_VALUE_LITERAL,
    TELEMETRY_METRIC_CONSTRUCTION,
    TELEMETRY_METRIC_NAME_LITERAL,
    TELEMETRY_SPAN_NAME_LITERAL,
    TEST_MODULE_NAMING,
]);

rustc_session::impl_lint_pass!(redundant_module_path::RedundantModulePath => [REDUNDANT_MODULE_PATH]);

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
