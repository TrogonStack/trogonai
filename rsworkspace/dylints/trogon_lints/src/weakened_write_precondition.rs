use clippy_utils::diagnostics::{span_lint_and_then, span_lint_hir_and_then};
use rustc_hir::def::{CtorOf, DefKind, Res};
use rustc_hir::{BodyId, CRATE_HIR_ID, ConstItemRhs, ExprKind, ImplItem, ImplItemKind};
use rustc_lint::{LateContext, Level};
use rustc_middle::lint::LintLevelSource;

use crate::WEAKENED_WRITE_PRECONDITION;

/// The associated const every decider declares.
const PRECONDITION_CONST: &str = "WRITE_PRECONDITION";
/// The enum it is declared with.
const PRECONDITION_ENUM: &str = "WritePrecondition";
/// The one variant that appends without checking anything first.
const UNCHECKED_VARIANT: &str = "Any";

const ESCAPE_HATCH: &str = "if appending unconditionally is deliberate, argue it in place: \
     `#[cfg_attr(dylint_lib = \"trogon_lints\", allow(weakened_write_precondition, reason = \"...\"))]`";

pub(crate) fn check_impl_item<'tcx>(cx: &LateContext<'tcx>, impl_item: &'tcx ImplItem<'tcx>) {
    if impl_item.span.from_expansion() || impl_item.ident.as_str() != PRECONDITION_CONST {
        return;
    }

    let ImplItemKind::Const(_, ConstItemRhs::Body(body_id)) = impl_item.kind else {
        return;
    };
    if !declares_unchecked_writes(cx, body_id) {
        return;
    }

    if let Some(reasoned) = allowed_at(cx, impl_item) {
        // An `allow` carrying a reason is the escape hatch working: the choice
        // was made and argued, which is all this lint asks of it.
        if reasoned {
            return;
        }

        // A bare `allow` silences the question instead of answering it, so the
        // report is levelled at the crate root, out of that attribute's reach.
        span_lint_hir_and_then(
            cx,
            WEAKENED_WRITE_PRECONDITION,
            CRATE_HIR_ID,
            impl_item.span,
            message(),
            |diag| {
                diag.help("the `allow` silencing this carries no `reason`, so the choice is turned off rather than argued");
                diag.help(ESCAPE_HATCH);
            },
        );
        return;
    }

    span_lint_and_then(cx, WEAKENED_WRITE_PRECONDITION, impl_item.span, message(), |diag| {
        diag.help(
            "name the invariant the append depends on instead: `NoStream` for a creation, `StreamExists` for \
             a transition, `StreamUnchanged` for a decision made from the state it read",
        );
        diag.help(ESCAPE_HATCH);
    });
}

fn message() -> String {
    format!("`{PRECONDITION_CONST}` is `{PRECONDITION_ENUM}::{UNCHECKED_VARIANT}`, so the append is unconditional")
}

/// Whether an attribute allows this lint here, and whether it gave a reason.
///
/// Read off the lint level rather than the attribute list, so `allow` and
/// `expect` and every spelling of them are one case.
fn allowed_at<'tcx>(cx: &LateContext<'tcx>, impl_item: &'tcx ImplItem<'tcx>) -> Option<bool> {
    let level = cx.tcx.lint_level_at_node(WEAKENED_WRITE_PRECONDITION, impl_item.hir_id());
    let LintLevelSource::Node { reason, .. } = level.src else {
        return None;
    };

    matches!(level.level, Level::Allow | Level::Expect).then(|| reason.is_some())
}

/// Whether the const initializer is `WritePrecondition::Any`.
///
/// Resolved through the variant rather than matched on the written path, so an
/// imported `Any` and a fully qualified one are the same declaration.
fn declares_unchecked_writes<'tcx>(cx: &LateContext<'tcx>, body_id: BodyId) -> bool {
    let value = cx.tcx.hir_body(body_id).value;
    let ExprKind::Path(qpath) = value.kind else {
        return false;
    };
    let variant = match cx.qpath_res(&qpath, value.hir_id) {
        Res::Def(DefKind::Ctor(CtorOf::Variant, _), ctor) => cx.tcx.parent(ctor),
        Res::Def(DefKind::Variant, variant) => variant,
        _ => return false,
    };

    cx.tcx.item_name(variant).as_str() == UNCHECKED_VARIANT
        && cx.tcx.item_name(cx.tcx.parent(variant)).as_str() == PRECONDITION_ENUM
}
