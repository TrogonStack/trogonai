use clippy_utils::diagnostics::span_lint_and_then;
use rustc_hir::{ItemKind, Stmt, StmtKind};
use rustc_lint::LateContext;
use rustc_span::SourceFile;

use crate::FUNCTION_LOCAL_MACRO_RULES;

pub(crate) fn check_stmt<'tcx>(cx: &LateContext<'tcx>, stmt: &'tcx Stmt<'tcx>) {
    if stmt.span.from_expansion() {
        return;
    }

    // A `macro_rules!` that surfaces as a statement is block-local by
    // construction: module-level macro definitions are items, never
    // statements. So reaching a `StmtKind::Item` that resolves to a macro
    // definition is the violation.
    let StmtKind::Item(item_id) = stmt.kind else {
        return;
    };

    let item = cx.tcx.hir_item(item_id);
    let ItemKind::Macro(ident, ..) = item.kind else {
        return;
    };

    let file = cx.tcx.sess.source_map().lookup_char_pos(stmt.span.lo()).file;
    if is_generated(&file) {
        return;
    }

    span_lint_and_then(
        cx,
        FUNCTION_LOCAL_MACRO_RULES,
        item.span,
        format!("`macro_rules! {ident}` declared inside a function body"),
        |diag| {
            diag.help(
                "move the macro to module scope; pass what it captured from the enclosing scope as macro arguments",
            );
        },
    );
}

fn is_generated(file: &SourceFile) -> bool {
    let Some(src) = &file.src else {
        return false;
    };
    src.lines().take(5).any(|line| line.contains("@generated"))
}
