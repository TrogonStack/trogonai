//! Ported from the `unstructured_log_fields` lint in
//! <https://github.com/li-kai/rust-lints>. The rule, its name, and the shape of
//! its exceptions are theirs; see this crate's README for the full credit.

use clippy_utils::diagnostics::span_lint_and_then;
use rustc_ast::LitKind;
use rustc_hir::{ConstItemRhs, Expr, ExprKind, ItemKind, StmtKind};
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

#[derive(Default)]
pub(crate) struct UnstructuredLogFields;

impl UnstructuredLogFields {
    pub(crate) fn check_expr<'tcx>(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        let ExprKind::Call(callee, args) = expr.kind else {
            return;
        };
        let Some(kind) = metadata_new_kind(cx, callee, args) else {
            return;
        };
        if kind.as_str() != "EVENT" {
            return;
        }
        let Some(fields) = args.get(FIELDS_ARGUMENT) else {
            return;
        };
        if !is_message_only(cx, fields) {
            return;
        }
        let Some(call_site) = tracing_macro_call_site(cx, expr.span) else {
            return;
        };
        if in_test_file(cx, call_site) {
            return;
        }
        // With `message` as the only field, every value the callsite captures
        // came from the format string, so a placeholder anywhere in the
        // invocation is a value that could have been a field. A message with no
        // placeholder captures nothing and has nothing to structure.
        let Ok(snippet) = cx.tcx.sess.source_map().span_to_snippet(call_site) else {
            return;
        };
        if !has_format_placeholder(&snippet) {
            return;
        }

        span_lint_and_then(
            cx,
            UNSTRUCTURED_LOG_FIELDS,
            call_site,
            "log event interpolates its values into the message instead of recording them as fields",
            |diag| {
                diag.help("pass the values as `tracing` fields, as in `tracing::info!(user_id, path, \"request handled\")`");
            },
        );
    }
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

/// Whether `snippet` contains a format placeholder. `{{` is an escaped brace
/// rather than a placeholder and does not count.
fn has_format_placeholder(snippet: &str) -> bool {
    let mut rest = snippet;
    while let Some(open) = rest.find('{') {
        rest = &rest[open + 1..];
        match rest.strip_prefix('{') {
            Some(after_escape) => rest = after_escape,
            None => return true,
        }
    }
    false
}
