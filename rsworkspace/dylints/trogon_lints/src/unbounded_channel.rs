use clippy_utils::diagnostics::span_lint_and_then;
use rustc_hir::def::{DefKind, Res};
use rustc_hir::{Expr, ExprKind};
use rustc_lint::LateContext;

use crate::UNBOUNDED_CHANNEL;
use crate::test_context::is_test_context;

/// A channel constructor that yields a queue with no capacity, and the bounded
/// constructor from the same family that replaces it.
///
/// Each entry is matched by owning crate, the module the function is declared
/// in, and the function name, so a bounded constructor that merely shares a
/// name with an unbounded one in another family (`tokio`'s
/// `mpsc::channel(capacity)` against `std`'s `mpsc::channel()`) is told apart
/// by its crate. `module` is `None` for the families that export the
/// constructor at the crate root.
struct UnboundedConstructor {
    krate: &'static str,
    module: Option<&'static str>,
    function: &'static str,
    replacement: &'static str,
}

const UNBOUNDED_CONSTRUCTORS: &[UnboundedConstructor] = &[
    // `std::sync::mpsc::channel` is the unbounded one of the pair;
    // `sync_channel` is what takes a capacity.
    UnboundedConstructor {
        krate: "std",
        module: Some("mpsc"),
        function: "channel",
        replacement: "std::sync::mpsc::sync_channel(capacity)",
    },
    UnboundedConstructor {
        krate: "tokio",
        module: Some("mpsc"),
        function: "unbounded_channel",
        replacement: "tokio::sync::mpsc::channel(capacity)",
    },
    // `futures::channel::mpsc` re-exports the `futures-channel` crate.
    UnboundedConstructor {
        krate: "futures_channel",
        module: Some("mpsc"),
        function: "unbounded",
        replacement: "futures::channel::mpsc::channel(capacity)",
    },
    UnboundedConstructor {
        krate: "flume",
        module: None,
        function: "unbounded",
        replacement: "flume::bounded(capacity)",
    },
    // `crossbeam::channel` re-exports the `crossbeam-channel` crate.
    UnboundedConstructor {
        krate: "crossbeam_channel",
        module: None,
        function: "unbounded",
        replacement: "crossbeam::channel::bounded(capacity)",
    },
    UnboundedConstructor {
        krate: "async_channel",
        module: None,
        function: "unbounded",
        replacement: "async_channel::bounded(capacity)",
    },
];

pub(crate) fn check_expr<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
    if expr.span.from_expansion() {
        return;
    }

    let ExprKind::Call(func, _) = expr.kind else {
        return;
    };
    let ExprKind::Path(qpath) = &func.kind else {
        return;
    };
    let Res::Def(DefKind::Fn, def_id) = cx.qpath_res(qpath, func.hir_id) else {
        return;
    };

    let Some(constructor) = UNBOUNDED_CONSTRUCTORS.iter().find(|candidate| {
        cx.tcx.crate_name(def_id.krate).as_str() == candidate.krate
            && cx.tcx.item_name(def_id).as_str() == candidate.function
            && candidate.module.is_none_or(|module| {
                cx.tcx.item_name(cx.tcx.parent(def_id)).as_str() == module
            })
    }) else {
        return;
    };

    if is_test_context(cx, expr.hir_id, expr.span) {
        return;
    }

    span_lint_and_then(
        cx,
        UNBOUNDED_CHANNEL,
        func.span,
        "unbounded channel created; a slow consumer queues messages until the process runs out of memory",
        |diag| {
            diag.help(format!(
                "give the queue an explicit capacity with `{}`, so a producer that outruns its consumer waits instead of allocating",
                constructor.replacement
            ));
            diag.note(
                "if the queue is bounded by something other than its capacity, opt out at the site with \
                 `#[cfg_attr(dylint_lib = \"trogon_lints\", allow(unbounded_channel, reason = \"...\"))]`",
            );
        },
    );
}
