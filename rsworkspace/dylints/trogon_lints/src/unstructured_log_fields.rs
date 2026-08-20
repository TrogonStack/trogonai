//! Ported from the `unstructured_log_fields` lint in
//! <https://github.com/li-kai/rust-lints>. The rule, its name, and the shape of
//! its exceptions are theirs; see this crate's README for the full credit.

use std::collections::HashSet;

use clippy_utils::diagnostics::span_lint_hir_and_then;
use rustc_ast::LitKind;
use rustc_hir::def::{DefKind, Res};
use rustc_hir::{ConstItemRhs, Expr, ExprKind, HirId, ItemKind, QPath, StmtKind, TyKind};
use rustc_lint::LateContext;
use rustc_span::{ExpnKind, Span, Symbol};

use crate::UNSTRUCTURED_LOG_FIELDS;
use crate::telemetry_literal::in_test_file;
use crate::tracing_metadata::metadata_new_kind;

/// Position of the `fields` parameter in `tracing_core::Metadata::new(name,
/// target, level, file, line, module_path, fields, kind)`.
const FIELDS_ARGUMENT: usize = 6;

/// The field `tracing` synthesizes for a macro's format-string message. A
/// callsite whose whole field set is this one field captured nothing but the
/// message.
const MESSAGE_FIELD: &str = "message";

/// A `tracing` callsite is two separate pieces of HIR: the callsite metadata,
/// which names the fields, and the message, which lowers to a `core::fmt`
/// constructor in the enclosing function. Neither piece alone says whether the
/// callsite interpolated a value that should have been a field, and they sit in
/// different bodies, so both are collected as they are visited and matched up
/// afterwards on the macro call site they share.
#[derive(Default)]
pub(crate) struct UnstructuredLogFields {
    message_only_events: HashSet<Span>,
    interpolating_messages: Vec<(Span, HirId)>,
}

impl UnstructuredLogFields {
    pub(crate) fn check_expr<'tcx>(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        let ExprKind::Call(callee, args) = expr.kind else {
            return;
        };

        if is_message_only_event(cx, callee, args) {
            if let Some(call_site) = tracing_macro_call_site(cx, expr.span)
                && !in_test_file(cx, call_site)
            {
                self.message_only_events.insert(call_site);
            }
            return;
        }

        if interpolates_values(cx, callee)
            && let Some(call_site) = tracing_macro_call_site(cx, expr.span)
        {
            // The `HirId` is the message's own, which lives in the enclosing
            // function rather than in the static holding the metadata, so an
            // `#[allow]` a reader would expect to apply still does.
            self.interpolating_messages.push((call_site, expr.hir_id));
        }
    }

    pub(crate) fn check_crate_post(&mut self, cx: &LateContext<'_>) {
        // Collection order follows the visitor, not the source, so the
        // diagnostics are ordered here to keep the output stable.
        self.interpolating_messages.sort_by_key(|(span, _)| span.lo());
        self.interpolating_messages.dedup_by_key(|(span, _)| *span);

        for (call_site, hir_id) in self.interpolating_messages.drain(..) {
            if !self.message_only_events.contains(&call_site) {
                continue;
            }

            span_lint_hir_and_then(
                cx,
                UNSTRUCTURED_LOG_FIELDS,
                hir_id,
                call_site,
                "log event interpolates its values into the message instead of recording them as fields",
                |diag| {
                    diag.help("pass the values as `tracing` fields, as in `tracing::info!(user_id, path, \"request handled\")`");
                },
            );
        }
    }
}

/// Whether a `Metadata::new` call describes an event whose whole field set is
/// the synthesized `message` field.
fn is_message_only_event<'tcx>(
    cx: &LateContext<'tcx>,
    callee: &'tcx Expr<'tcx>,
    args: &'tcx [Expr<'tcx>],
) -> bool {
    let Some(kind) = metadata_new_kind(cx, callee, args) else {
        return false;
    };
    if kind.as_str() != "EVENT" {
        return false;
    }
    args.get(FIELDS_ARGUMENT)
        .is_some_and(|fields| is_message_only(cx, fields))
}

/// Whether `callee` is the `core::fmt::Arguments` constructor that a format
/// string carrying at least one interpolated value lowers to. A format string
/// that captures nothing lowers to `Arguments::from_str` instead, so the
/// constructor answers on its own whether the message holds values, without
/// reading the invocation's text. Reading the text cannot answer it: a brace in
/// a `target:` or `parent:` argument, or the braces of a `tracing::info! { .. }`
/// invocation, are not message placeholders.
fn interpolates_values(cx: &LateContext<'_>, callee: &Expr<'_>) -> bool {
    let ExprKind::Path(QPath::TypeRelative(self_ty, segment)) = &callee.kind else {
        return false;
    };
    if segment.ident.name.as_str() != "new" {
        return false;
    }
    let TyKind::Path(QPath::Resolved(_, path)) = self_ty.kind else {
        return false;
    };
    let Res::Def(DefKind::Struct, did) = path.res else {
        return false;
    };
    cx.tcx.crate_name(did.krate).as_str() == "core" && cx.tcx.item_name(did).as_str() == "Arguments"
}

/// Whether the `fields` argument of a `Metadata::new` call names `message` and
/// nothing else. `tracing` lowers the field set to a `FieldSet::new(&["a",
/// "b"], _)` call whose first argument is an array of the field names, so the
/// structured fields a callsite declares are readable straight off the
/// expansion.
fn is_message_only(cx: &LateContext<'_>, fields: &Expr<'_>) -> bool {
    let ExprKind::Call(_, args) = fields.kind else {
        return false;
    };
    let Some(names) = args.first() else {
        return false;
    };
    let ExprKind::AddrOf(_, _, array) = names.kind else {
        return false;
    };
    let ExprKind::Array([only]) = array.kind else {
        return false;
    };
    field_name(cx, only).is_some_and(|name| name.as_str() == MESSAGE_FIELD)
}

/// The field name an entry of the `FieldSet` array spells. A plain invocation
/// leaves the name a bare string literal; one carrying a `target:` or `parent:`
/// directive expands through a different arm that wraps the name in a block
/// declaring `const NAME: FieldName<_> = FieldName::new("...")`.
fn field_name(cx: &LateContext<'_>, entry: &Expr<'_>) -> Option<Symbol> {
    match entry.kind {
        ExprKind::Lit(_) => string_literal(entry),
        ExprKind::Block(block, _) => block.stmts.iter().find_map(|stmt| {
            let StmtKind::Item(item_id) = stmt.kind else {
                return None;
            };
            let ItemKind::Const(_, _, _, ConstItemRhs::Body(body_id)) =
                cx.tcx.hir_item(item_id).kind
            else {
                return None;
            };
            let ExprKind::Call(_, args) = cx.tcx.hir_body(body_id).value.kind else {
                return None;
            };
            args.first().and_then(|arg| string_literal(arg))
        }),
        _ => None,
    }
}

fn string_literal(expr: &Expr<'_>) -> Option<Symbol> {
    let ExprKind::Lit(lit) = expr.kind else {
        return None;
    };
    match lit.node {
        LitKind::Str(name, _) => Some(name),
        _ => None,
    }
}

/// The call site of the outermost `tracing` macro that `span` expands from, or
/// `None` when `span` comes from somewhere else. The metadata this lint reads
/// is built several expansions deep (`info!` to `event!` to `callsite2!` to
/// `metadata!`), so the chain has to be walked back out to the invocation a
/// reader can see and edit.
fn tracing_macro_call_site(cx: &LateContext<'_>, span: Span) -> Option<Span> {
    let mut current = span;
    let mut call_site = None;
    while current.from_expansion() {
        let expansion = current.ctxt().outer_expn_data();
        if matches!(expansion.kind, ExpnKind::Macro(..))
            && let Some(def_id) = expansion.macro_def_id
            && cx.tcx.crate_name(def_id.krate).as_str() == "tracing"
        {
            call_site = Some(expansion.call_site);
        }
        current = expansion.call_site;
    }
    call_site
}
