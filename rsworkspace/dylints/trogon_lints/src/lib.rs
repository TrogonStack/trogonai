#![feature(rustc_private)]

extern crate rustc_hir;
extern crate rustc_lint;
extern crate rustc_session;

mod error_string_comparison;
mod manual_error_impl;

use rustc_hir::{Expr, Item, LetStmt};
use rustc_lint::{LateContext, LateLintPass, LintStore};

dylint_linting::dylint_library!();

#[unsafe(no_mangle)]
pub fn register_lints(sess: &rustc_session::Session, lint_store: &mut LintStore) {
    dylint_linting::init_config(sess);
    lint_store.register_lints(&[ERROR_STRING_COMPARISON, MANUAL_ERROR_IMPL]);
    lint_store.register_late_pass(|_| Box::<TrogonLints>::default());
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
    Warn,
    "error display strings must not drive behavior",
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
    Warn,
    "implement `std::error::Error` with the thiserror derive, not by hand",
}

#[derive(Default)]
struct TrogonLints {
    error_string_comparison: error_string_comparison::ErrorStringComparison,
}

impl<'tcx> LateLintPass<'tcx> for TrogonLints {
    fn check_local(&mut self, cx: &LateContext<'tcx>, local: &'tcx LetStmt<'tcx>) {
        self.error_string_comparison.check_local(cx, local);
    }

    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        self.error_string_comparison.check_expr(cx, expr);
    }

    fn check_item(&mut self, cx: &LateContext<'tcx>, item: &'tcx Item<'tcx>) {
        manual_error_impl::check_item(cx, item);
    }
}

rustc_session::impl_lint_pass!(TrogonLints => [ERROR_STRING_COMPARISON, MANUAL_ERROR_IMPL]);

#[test]
fn ui() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui");
}
