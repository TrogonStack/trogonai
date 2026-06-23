use clippy_utils::diagnostics::span_lint_and_then;
use rustc_hir::{Item, ItemKind};
use rustc_lint::LateContext;
use rustc_span::SourceFile;

use crate::INLINE_MODULE_BLOCK;

pub(crate) fn check_item<'tcx>(cx: &LateContext<'tcx>, item: &'tcx Item<'tcx>) {
    if item.span.from_expansion() {
        return;
    }

    let ItemKind::Mod(ident, module) = item.kind else {
        return;
    };

    // `mod foo;` puts the body in `foo.rs` / `foo/mod.rs`, so the declaration
    // and the body live in different files. An inline `mod foo { ... }` keeps
    // both in the same file: that is the violation.
    let source_map = cx.tcx.sess.source_map();
    let decl_file = source_map.lookup_char_pos(item.span.lo()).file;
    let body_file = source_map.lookup_char_pos(module.spans.inner_span.lo()).file;
    if decl_file.name != body_file.name {
        return;
    }

    // Generated files (proto codegen, etc.) emit inline module trees that are
    // not hand-authored and must not be reshaped to match this policy. Skip any
    // file carrying the conventional `@generated` marker near its top.
    if is_generated(&body_file) {
        return;
    }

    span_lint_and_then(
        cx,
        INLINE_MODULE_BLOCK,
        item.span.with_hi(module.spans.inner_span.lo()),
        format!("inline module `{ident}` declared with a body block"),
        |diag| {
            diag.help(format!(
                "move the contents into `{ident}.rs` (or `{ident}/mod.rs`) and declare it as `mod {ident};`"
            ));
        },
    );
}

fn is_generated(file: &SourceFile) -> bool {
    let Some(src) = &file.src else {
        return false;
    };
    src.lines().take(5).any(|line| line.contains("@generated"))
}
