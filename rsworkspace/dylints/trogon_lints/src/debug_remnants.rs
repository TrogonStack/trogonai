//! Ported from the `debug_remnants` lint in
//! <https://github.com/li-kai/rust-lints>. The rule, its name, and the shape of
//! its exceptions are theirs; see this crate's README for the full credit.

use std::collections::HashSet;

use clippy_utils::diagnostics::span_lint_and_then;
use rustc_hir::Expr;
use rustc_lint::LateContext;
use rustc_span::hygiene::MacroKind;
use rustc_span::{ExpnKind, Span};

use crate::DEBUG_REMNANTS;
use crate::test_context::is_test_context;

/// A `std` printing macro and the `tracing` level that carries the same
/// information as a structured event. The level follows the stream and the
/// intent the macro implies: stdout is progress a reader wants by default,
/// stderr is something that went wrong, and `dbg!` is inspection detail.
struct DebugMacro {
    name: &'static str,
    level: &'static str,
}

/// `eprint!` is deliberately absent. Writing to stderr without a newline is how
/// a terminal program draws a prompt or a progress line, which a log event
/// cannot replace, so it is not a leftover debugging statement.
const DEBUG_MACROS: &[DebugMacro] = &[
    DebugMacro {
        name: "print",
        level: "info",
    },
    DebugMacro {
        name: "println",
        level: "info",
    },
    DebugMacro {
        name: "eprintln",
        level: "warn",
    },
    DebugMacro {
        name: "dbg",
        level: "debug",
    },
];

#[derive(Default)]
pub(crate) struct DebugRemnants {
    /// Every expression the macro expands to carries that expansion in its
    /// span, so the pass would otherwise report the same invocation once per
    /// node it produced. Report the first node reached and remember the call
    /// site.
    reported: HashSet<Span>,
}

impl DebugRemnants {
    pub(crate) fn check_expr<'tcx>(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        let Some((debug_macro, call_site)) = debug_macro_call(cx, expr.span) else {
            return;
        };

        if !self.reported.insert(call_site) {
            return;
        }

        if is_test_context(cx, expr.hir_id, call_site) {
            return;
        }

        span_lint_and_then(
            cx,
            DEBUG_REMNANTS,
            call_site,
            format!(
                "debug remnant: `{}!` writes straight to the process's own stdio",
                debug_macro.name
            ),
            |diag| {
                diag.help(format!(
                    "record it as a structured event with `tracing::{}!(...)`, so it carries named fields and a level and reaches the configured subscriber",
                    debug_macro.level
                ));
                diag.note(
                    "if the write is the program's own output rather than a diagnostic, opt out at the site with \
                     `#[cfg_attr(dylint_lib = \"trogon_lints\", allow(debug_remnants, reason = \"...\"))]`",
                );
            },
        );
    }
}

/// The printing macro `span` was expanded from, together with the call site a
/// reader can see and edit.
///
/// A macro that reaches its printing through another macro (`dbg!` goes through
/// `dbg_internal!`, which in turn calls `eprintln!`) puts several frames between
/// the invocation and the expression, so the chain is walked back out and the
/// outermost printing macro on it answers for the site. The invocation has to be
/// hand-written: an expansion whose own call site comes from a further
/// expansion is code the caller did not write, so a macro of someone else's that
/// happens to print is left alone. Each frame is identified by its definition
/// rather than by name, so a `println!` of one's own is not mistaken for
/// `std`'s.
fn debug_macro_call(cx: &LateContext<'_>, span: Span) -> Option<(&'static DebugMacro, Span)> {
    let mut current = span;
    let mut outermost = None;

    while current.from_expansion() {
        let expansion = current.ctxt().outer_expn_data();
        if let ExpnKind::Macro(MacroKind::Bang, name) = expansion.kind
            && let Some(def_id) = expansion.macro_def_id
            && cx.tcx.crate_name(def_id.krate).as_str() == "std"
            && let Some(debug_macro) = DEBUG_MACROS
                .iter()
                .find(|candidate| candidate.name == unqualified(name.as_str()))
        {
            outermost = Some((debug_macro, expansion.call_site));
        }
        current = expansion.call_site;
    }

    let (debug_macro, call_site) = outermost?;
    if call_site.from_expansion() {
        return None;
    }

    Some((debug_macro, call_site))
}

/// The macro's own name, without the path an invocation may have spelled it
/// with (`std::println` and `$crate::println` are both `println`).
fn unqualified(name: &str) -> &str {
    match name.rsplit_once("::") {
        Some((_, unqualified)) => unqualified,
        None => name,
    }
}
