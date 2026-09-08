use clippy_utils::diagnostics::span_lint_and_then;
use rustc_ast::{Attribute, MetaItemInner};
use rustc_lint::{EarlyContext, EarlyLintPass};

use crate::SERDE_JSON_MACRO_ALLOW_WITHOUT_REASON;

/// Lint level attributes are consumed before HIR, and `cfg_attr(dylint_lib =
/// "trogon_lints", ...)` is expanded during macro expansion, so the suppression
/// is only visible to an early (AST) pass.
pub(crate) struct SerdeJsonMacroAllowWithoutReason;

impl EarlyLintPass for SerdeJsonMacroAllowWithoutReason {
    fn check_attribute(&mut self, cx: &EarlyContext<'_>, attr: &Attribute) {
        // `expect` suppresses the diagnostic exactly like `allow` does, so it
        // owes the same justification.
        let Some(level) = attr.name() else {
            return;
        };
        if !matches!(level.as_str(), "allow" | "expect") {
            return;
        }

        let Some(items) = attr.meta_item_list() else {
            return;
        };
        if !items.iter().any(is_serde_json_macro) {
            return;
        }
        if items.iter().any(is_stated_reason) {
            return;
        }

        span_lint_and_then(
            cx,
            SERDE_JSON_MACRO_ALLOW_WITHOUT_REASON,
            attr.span,
            format!("`serde_json_macro` suppressed by `{level}` without a stated reason"),
            |diag| {
                diag.help(format!(
                    "state why the payload cannot be a `Serialize` type: `{level}(serde_json_macro, reason = \"...\")`"
                ));
            },
        );
    }
}

fn is_serde_json_macro(item: &MetaItemInner) -> bool {
    item.is_word()
        && item
            .name()
            .is_some_and(|name| name.as_str() == "serde_json_macro")
}

/// A `reason` that says nothing is not a justification, so an empty string is
/// treated as a missing one.
fn is_stated_reason(item: &MetaItemInner) -> bool {
    item.name().is_some_and(|name| name.as_str() == "reason")
        && item
            .value_str()
            .is_some_and(|reason| !reason.as_str().trim().is_empty())
}
