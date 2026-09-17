use std::collections::HashSet;

use clippy_utils::consts::{ConstEvalCtxt, Constant, const_item_rhs_to_expr};
use clippy_utils::diagnostics::span_lint_and_help;
use clippy_utils::is_inside_always_const_context;
use clippy_utils::macros::{find_assert_args, root_macro_call_first_node};
use rustc_hir::def::{DefKind, Res};
use rustc_hir::def_id::LocalDefId;
use rustc_hir::{BinOpKind, Expr, ExprKind, ItemKind, Node, UnOp};
use rustc_lint::LateContext;
use rustc_middle::ty;
use rustc_span::{Span, sym};

use crate::ASSERTIONS_ON_FIXED_LITERALS;

struct FixedValue {
    value: Constant,
    source: Option<LocalDefId>,
}

pub(crate) fn check_expr<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
    if let Some(call) = root_macro_call_first_node(cx, expr)
        && matches!(
            cx.tcx.get_diagnostic_name(call.def_id),
            Some(sym::assert_macro | sym::debug_assert_macro)
        )
        && is_inside_always_const_context(cx.tcx, expr.hir_id)
        && is_handwritten(cx, call.span)
        && let Some((condition, _)) = find_assert_args(cx, expr, call.expn)
        && predicate(cx, condition) == Some(true)
    {
        span_lint_and_help(
            cx,
            ASSERTIONS_ON_FIXED_LITERALS,
            call.span,
            "this assertion only confirms a fixed literal",
            None,
            "remove the assertion; validate supplied values or relationships between independent constants instead",
        );
    }
}

fn predicate<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) -> Option<bool> {
    match expr.kind {
        ExprKind::Unary(UnOp::Not, inner) => predicate(cx, inner).map(|value| !value),
        ExprKind::Binary(op, left, right) if op.node.is_comparison() => {
            let left_value = fixed_value(cx, left)?;
            let right_value = fixed_value(cx, right)?;
            if let (Some(left), Some(right)) = (left_value.source, right_value.source)
                && left != right
            {
                return None;
            }
            let ordering = Constant::partial_cmp(
                cx.tcx,
                cx.typeck_results().expr_ty(left),
                &left_value.value,
                &right_value.value,
            )?;
            Some(match op.node {
                BinOpKind::Eq => ordering.is_eq(),
                BinOpKind::Ne => !ordering.is_eq(),
                BinOpKind::Lt => ordering.is_lt(),
                BinOpKind::Le => !ordering.is_gt(),
                BinOpKind::Gt => ordering.is_gt(),
                BinOpKind::Ge => !ordering.is_lt(),
                _ => return None,
            })
        }
        _ => match fixed_value(cx, expr)?.value {
            Constant::Bool(value) => Some(value),
            _ => None,
        },
    }
}

fn fixed_value<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) -> Option<FixedValue> {
    if let ExprKind::MethodCall(_, receiver, [], _) = expr.kind {
        if !is_handwritten(cx, expr.span) {
            return None;
        }
        let method = cx.typeck_results().type_dependent_def_id(expr.hir_id)?;
        let implementation = cx.tcx.impl_of_assoc(method)?;
        if cx.tcx.crate_name(method.krate) != sym::core
            || !matches!(
                cx.tcx.type_of(implementation).instantiate_identity().kind(),
                ty::Str | ty::Slice(_)
            )
        {
            return None;
        }
        let receiver = literal_value(cx, receiver)?;
        let len = match &receiver.value {
            Constant::Str(value) => value.len(),
            Constant::Binary(value) => value.len(),
            _ => return None,
        };
        let value = match cx.tcx.item_name(method).as_str() {
            "len" => Constant::Int(len as u128),
            "is_empty" => Constant::Bool(len == 0),
            _ => return None,
        };
        Some(FixedValue {
            value,
            source: receiver.source,
        })
    } else {
        literal_value(cx, expr)
    }
}

fn literal_value<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) -> Option<FixedValue> {
    let mut visited = HashSet::new();
    let literal = source_literal(cx, expr, &mut visited)?;
    let source = local_constant(cx, expr);
    let typeck = cx.tcx.typeck(literal.hir_id.owner.def_id);
    let value = ConstEvalCtxt::with_env(cx.tcx, cx.typing_env(), typeck).eval(literal)?;
    Some(FixedValue { value, source })
}

fn source_literal<'tcx>(
    cx: &LateContext<'tcx>,
    expr: &'tcx Expr<'tcx>,
    visited: &mut HashSet<LocalDefId>,
) -> Option<&'tcx Expr<'tcx>> {
    if !is_handwritten(cx, expr.span) {
        return None;
    }
    match expr.kind {
        ExprKind::Lit(_) => Some(expr),
        ExprKind::Unary(UnOp::Neg, inner) if matches!(inner.kind, ExprKind::Lit(_)) => {
            is_handwritten(cx, inner.span).then_some(expr)
        }
        ExprKind::Path(_) => {
            let id = local_constant(cx, expr)?;
            if !visited.insert(id) {
                return None;
            }
            let Node::Item(item) = cx.tcx.hir_node_by_def_id(id) else {
                return None;
            };
            let ItemKind::Const(_, _, _, rhs) = item.kind else {
                return None;
            };
            if !is_handwritten(cx, item.span) {
                return None;
            }
            source_literal(cx, const_item_rhs_to_expr(cx.tcx, rhs)?, visited)
        }
        _ => None,
    }
}

fn local_constant(cx: &LateContext<'_>, expr: &Expr<'_>) -> Option<LocalDefId> {
    let ExprKind::Path(path) = &expr.kind else {
        return None;
    };
    let typeck = cx.tcx.typeck(expr.hir_id.owner.def_id);
    let Res::Def(DefKind::Const { .. }, id) = typeck.qpath_res(path, expr.hir_id) else {
        return None;
    };
    id.as_local()
}

fn is_handwritten(cx: &LateContext<'_>, span: Span) -> bool {
    if span.from_expansion() {
        return false;
    }
    let file = cx.tcx.sess.source_map().lookup_char_pos(span.lo()).file;
    !file.src.as_ref().is_some_and(|source| {
        source
            .lines()
            .take(5)
            .any(|line| line.contains("@generated") || line.contains("DO NOT EDIT"))
    })
}
