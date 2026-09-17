use clippy_utils::diagnostics::span_lint_and_then;
use clippy_utils::sym;
use clippy_utils::ty::implements_trait;
use rustc_hir::def::Res;
use rustc_hir::{BinOpKind, Expr, ExprKind, HirId, LetExpr, LetStmt, Pat, PatKind, QPath};
use rustc_lint::LateContext;
use std::collections::HashSet;

use crate::ERROR_STRING_COMPARISON;

#[derive(Default)]
pub(crate) struct ErrorStringComparison {
    error_strings: HashSet<HirId>,
}

impl ErrorStringComparison {
    pub(crate) fn check_local<'tcx>(&mut self, cx: &LateContext<'tcx>, local: &'tcx LetStmt<'tcx>) {
        if let Some(init) = local.init {
            self.bind_pattern_from_expr(cx, local.pat, init);
        }
    }

    pub(crate) fn check_expr<'tcx>(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        match expr.kind {
            ExprKind::Assign(left, right, _) => {
                self.bind_place_from_expr(cx, left, right);
            }
            ExprKind::Let(let_expr) => {
                self.check_let(cx, let_expr);
            }
            ExprKind::Binary(op, left, right)
                if matches!(op.node, BinOpKind::Eq | BinOpKind::Ne)
                    && (self.is_error_string(cx, left) || self.is_error_string(cx, right)) =>
            {
                lint(cx, expr);
            }
            ExprKind::MethodCall(segment, receiver, _, _)
                if is_string_probe(segment.ident.name.as_str()) && self.is_error_string(cx, receiver) =>
            {
                lint(cx, expr);
            }
            ExprKind::MethodCall(segment, _, args, _)
                if is_string_probe(segment.ident.name.as_str())
                    && args.iter().any(|arg| self.is_error_string(cx, arg)) =>
            {
                lint(cx, expr);
            }
            ExprKind::MethodCall(segment, _, args, _)
                if segment.ident.name.as_str() == "is_match"
                    && args.iter().any(|arg| self.is_error_string(cx, arg)) =>
            {
                lint(cx, expr);
            }
            _ => {}
        }
    }

    fn check_let<'tcx>(&mut self, cx: &LateContext<'tcx>, let_expr: &'tcx LetExpr<'tcx>) {
        self.bind_pattern_from_expr(cx, let_expr.pat, let_expr.init);
    }

    fn bind_pattern_from_expr<'tcx>(&mut self, cx: &LateContext<'tcx>, pattern: &Pat<'tcx>, expr: &'tcx Expr<'tcx>) {
        if self.is_error_string(cx, expr) {
            bind_pattern(&mut self.error_strings, pattern);
            return;
        }

        match (pattern.kind, expr.kind) {
            (PatKind::Tuple(patterns, _), ExprKind::Tup(exprs)) => {
                for (pattern, expr) in patterns.iter().zip(exprs) {
                    self.bind_pattern_from_expr(cx, pattern, expr);
                }
            }
            (PatKind::TupleStruct(pattern_path, patterns, _), ExprKind::Call(callee, exprs))
                if is_same_constructor(cx, pattern, &pattern_path, callee) =>
            {
                for (pattern, expr) in patterns.iter().zip(exprs) {
                    self.bind_pattern_from_expr(cx, pattern, expr);
                }
            }
            (PatKind::Slice(before, slice, after), ExprKind::Array(exprs)) => {
                let (before_exprs, rest) = exprs.split_at(before.len().min(exprs.len()));
                for (pattern, expr) in before.iter().zip(before_exprs) {
                    self.bind_pattern_from_expr(cx, pattern, expr);
                }

                if let Some(slice) = slice {
                    let after_len = after.len().min(rest.len());
                    let (slice_exprs, after_exprs) = rest.split_at(rest.len() - after_len);
                    for expr in slice_exprs {
                        self.bind_pattern_from_expr(cx, slice, expr);
                    }
                    for (pattern, expr) in after.iter().zip(after_exprs) {
                        self.bind_pattern_from_expr(cx, pattern, expr);
                    }
                } else {
                    let after_len = after.len().min(rest.len());
                    let after_exprs = &rest[rest.len() - after_len..];
                    for (pattern, expr) in after.iter().zip(after_exprs) {
                        self.bind_pattern_from_expr(cx, pattern, expr);
                    }
                }
            }
            (PatKind::Binding(_, hir_id, _, subpattern), _) => {
                self.error_strings.remove(&hir_id);
                if let Some(subpattern) = subpattern {
                    self.bind_pattern_from_expr(cx, subpattern, expr);
                }
            }
            (PatKind::Ref(inner, _, _) | PatKind::Box(inner) | PatKind::Deref(inner), _) => {
                self.bind_pattern_from_expr(cx, inner, expr);
            }
            _ => clear_pattern(&mut self.error_strings, pattern),
        }
    }

    fn bind_place_from_expr<'tcx>(&mut self, cx: &LateContext<'tcx>, place: &'tcx Expr<'tcx>, expr: &'tcx Expr<'tcx>) {
        if self.is_error_string(cx, expr) {
            bind_place(&mut self.error_strings, place);
            return;
        }

        match (place.kind, expr.kind) {
            (ExprKind::Tup(places), ExprKind::Tup(exprs)) => {
                for (place, expr) in places.iter().zip(exprs) {
                    self.bind_place_from_expr(cx, place, expr);
                }
            }
            _ => clear_place(&mut self.error_strings, place),
        }
    }

    fn is_error_string<'tcx>(&self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) -> bool {
        match expr.kind {
            ExprKind::Path(QPath::Resolved(_, path)) => match path.res {
                Res::Local(hir_id) => self.error_strings.contains(&hir_id),
                _ => false,
            },
            ExprKind::MethodCall(segment, receiver, _, _) if segment.ident.name.as_str() == "to_string" => {
                self.receiver_implements_error(cx, expr, receiver)
            }
            ExprKind::MethodCall(segment, receiver, _, _) if is_string_forwarder(segment.ident.name.as_str()) => {
                self.is_error_string(cx, receiver)
            }
            ExprKind::AddrOf(_, _, inner) | ExprKind::DropTemps(inner) | ExprKind::Use(inner, _) => {
                self.is_error_string(cx, inner)
            }
            ExprKind::Block(block, _) => block.expr.is_some_and(|inner| self.is_error_string(cx, inner)),
            ExprKind::If(_, then_expr, else_expr) => {
                self.is_error_string(cx, then_expr)
                    || else_expr.is_some_and(|else_expr| self.is_error_string(cx, else_expr))
            }
            ExprKind::Match(_, arms, _) => arms.iter().any(|arm| self.is_error_string(cx, arm.body)),
            _ => false,
        }
    }

    fn receiver_implements_error<'tcx>(
        &self,
        cx: &LateContext<'tcx>,
        call: &'tcx Expr<'tcx>,
        receiver: &'tcx Expr<'tcx>,
    ) -> bool {
        let Some(to_string_def_id) = cx.typeck_results().type_dependent_def_id(call.hir_id) else {
            return false;
        };

        if !cx.tcx.is_diagnostic_item(sym::to_string_method, to_string_def_id) {
            return false;
        }

        let Some(error_def_id) = cx.tcx.get_diagnostic_item(sym::Error) else {
            return false;
        };

        let receiver_ty = cx.typeck_results().expr_ty_adjusted(receiver);
        implements_trait(cx, receiver_ty, error_def_id, &[])
    }
}

fn is_same_constructor<'tcx>(
    cx: &LateContext<'tcx>,
    pattern: &Pat<'tcx>,
    pattern_path: &QPath<'tcx>,
    callee: &'tcx Expr<'tcx>,
) -> bool {
    let ExprKind::Path(callee_path) = callee.kind else {
        return false;
    };

    cx.qpath_res(pattern_path, pattern.hir_id) == cx.qpath_res(&callee_path, callee.hir_id)
}

fn bind_pattern(error_strings: &mut HashSet<HirId>, pattern: &Pat<'_>) {
    match pattern.kind {
        PatKind::Binding(_, hir_id, _, subpattern) => {
            error_strings.insert(hir_id);
            if let Some(subpattern) = subpattern {
                bind_pattern(error_strings, subpattern);
            }
        }
        PatKind::TupleStruct(_, fields, _) | PatKind::Tuple(fields, _) => {
            for field in fields {
                bind_pattern(error_strings, field);
            }
        }
        PatKind::Slice(before, slice, after) => {
            for field in before {
                bind_pattern(error_strings, field);
            }
            if let Some(slice) = slice {
                bind_pattern(error_strings, slice);
            }
            for field in after {
                bind_pattern(error_strings, field);
            }
        }
        PatKind::Struct(_, fields, _) => {
            for field in fields {
                bind_pattern(error_strings, field.pat);
            }
        }
        PatKind::Or(patterns) => {
            for pattern in patterns {
                bind_pattern(error_strings, pattern);
            }
        }
        PatKind::Ref(inner, _, _) | PatKind::Box(inner) | PatKind::Deref(inner) => bind_pattern(error_strings, inner),
        _ => {}
    }
}

fn clear_pattern(error_strings: &mut HashSet<HirId>, pattern: &Pat<'_>) {
    match pattern.kind {
        PatKind::Binding(_, hir_id, _, subpattern) => {
            error_strings.remove(&hir_id);
            if let Some(subpattern) = subpattern {
                clear_pattern(error_strings, subpattern);
            }
        }
        PatKind::TupleStruct(_, fields, _) | PatKind::Tuple(fields, _) => {
            for field in fields {
                clear_pattern(error_strings, field);
            }
        }
        PatKind::Slice(before, slice, after) => {
            for field in before {
                clear_pattern(error_strings, field);
            }
            if let Some(slice) = slice {
                clear_pattern(error_strings, slice);
            }
            for field in after {
                clear_pattern(error_strings, field);
            }
        }
        PatKind::Struct(_, fields, _) => {
            for field in fields {
                clear_pattern(error_strings, field.pat);
            }
        }
        PatKind::Or(patterns) => {
            for pattern in patterns {
                clear_pattern(error_strings, pattern);
            }
        }
        PatKind::Ref(inner, _, _) | PatKind::Box(inner) | PatKind::Deref(inner) => clear_pattern(error_strings, inner),
        _ => {}
    }
}

fn bind_place(error_strings: &mut HashSet<HirId>, place: &Expr<'_>) {
    match place.kind {
        ExprKind::Path(QPath::Resolved(_, path)) => {
            if let Res::Local(hir_id) = path.res {
                error_strings.insert(hir_id);
            }
        }
        ExprKind::Tup(places) => {
            for place in places {
                bind_place(error_strings, place);
            }
        }
        ExprKind::AddrOf(_, _, inner) | ExprKind::DropTemps(inner) | ExprKind::Use(inner, _) => {
            bind_place(error_strings, inner)
        }
        _ => {}
    }
}

fn clear_place(error_strings: &mut HashSet<HirId>, place: &Expr<'_>) {
    match place.kind {
        ExprKind::Path(QPath::Resolved(_, path)) => {
            if let Res::Local(hir_id) = path.res {
                error_strings.remove(&hir_id);
            }
        }
        ExprKind::Tup(places) => {
            for place in places {
                clear_place(error_strings, place);
            }
        }
        ExprKind::AddrOf(_, _, inner) | ExprKind::DropTemps(inner) | ExprKind::Use(inner, _) => {
            clear_place(error_strings, inner)
        }
        _ => {}
    }
}

fn is_string_probe(method: &str) -> bool {
    matches!(method, "contains" | "starts_with" | "ends_with" | "eq" | "ne")
}

fn is_string_forwarder(method: &str) -> bool {
    matches!(
        method,
        "as_ref"
            | "as_str"
            | "clone"
            | "to_owned"
            | "trim"
            | "trim_end"
            | "trim_start"
            | "to_lowercase"
            | "to_uppercase"
    )
}

fn lint(cx: &LateContext<'_>, expr: &Expr<'_>) {
    span_lint_and_then(
        cx,
        ERROR_STRING_COMPARISON,
        expr.span,
        "error display string used for a semantic comparison",
        |diag| {
            diag.help("compare typed error variants, typed categories, or value objects instead");
        },
    );
}
