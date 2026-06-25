use clippy_utils::diagnostics::span_lint_and_then;
use rustc_hir::{ItemKind, Stmt, StmtKind};
use rustc_lint::LateContext;
use rustc_span::Span;
use std::collections::HashSet;

use crate::FUNCTION_LOCAL_USE;

#[derive(Default)]
pub(crate) struct FunctionLocalUse {
    reported: HashSet<Span>,
}

impl FunctionLocalUse {
    pub(crate) fn check_stmt<'tcx>(&mut self, cx: &LateContext<'tcx>, stmt: &'tcx Stmt<'tcx>) {
        if stmt.span.from_expansion() {
            return;
        }

        // A `use` that surfaces as a statement is block-local by construction:
        // module-level imports are items, never statements. So reaching a
        // `StmtKind::Item` that resolves to a `use` is the violation.
        let StmtKind::Item(item_id) = stmt.kind else {
            return;
        };

        if !matches!(cx.tcx.hir_item(item_id).kind, ItemKind::Use(..)) {
            return;
        }

        // `use a::{b, c};` lowers to several HIR items (a list stem plus one per
        // leaf), each surfacing as its own statement that shares this span.
        // Report the statement once.
        if !self.reported.insert(stmt.span) {
            return;
        }

        span_lint_and_then(
            cx,
            FUNCTION_LOCAL_USE,
            stmt.span,
            "`use` declared inside a function body",
            |diag| {
                diag.help("move the import to module level (a `use` at the top of the file or module)");
            },
        );
    }
}
