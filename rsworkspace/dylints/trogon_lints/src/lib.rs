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
mod telemetry_attribute_literal;
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
        TELEMETRY_ATTRIBUTE_LITERAL,
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

#[derive(Default)]
struct TrogonLints {
    error_string_comparison: error_string_comparison::ErrorStringComparison,
    function_local_use: function_local_use::FunctionLocalUse,
    telemetry_attribute_literal: telemetry_attribute_literal::TelemetryAttributeLiteral,
}

impl<'tcx> LateLintPass<'tcx> for TrogonLints {
    fn check_local(&mut self, cx: &LateContext<'tcx>, local: &'tcx LetStmt<'tcx>) {
        self.error_string_comparison.check_local(cx, local);
    }

    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        self.error_string_comparison.check_expr(cx, expr);
        self.telemetry_attribute_literal.check_expr(cx, expr);
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
    TELEMETRY_ATTRIBUTE_LITERAL,
    TEST_MODULE_NAMING,
]);

rustc_session::impl_lint_pass!(redundant_module_path::RedundantModulePath => [REDUNDANT_MODULE_PATH]);

#[test]
fn ui() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui");
}

// `telemetry_attribute_literal` gates on the real `tracing::Span` type, so its
// fixture lives as an example target where it can depend on `tracing`; the
// dependency-free `ui` directory cannot.
#[test]
fn ui_telemetry_attribute_literal() {
    dylint_testing::ui_test_example(env!("CARGO_PKG_NAME"), "telemetry_attribute_literal");
}
