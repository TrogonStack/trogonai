use clippy_utils::diagnostics::span_lint_hir_and_then;
use clippy_utils::sym;
use rustc_hir::{Item, ItemKind};
use rustc_lint::LateContext;

use crate::ERROR_TYPE_NAMING;

pub(crate) fn check_item<'tcx>(cx: &LateContext<'tcx>, item: &'tcx Item<'tcx>) {
    let ItemKind::Impl(impl_) = item.kind else {
        return;
    };

    let Some(of_trait) = impl_.of_trait else {
        return;
    };

    let Some(trait_def_id) = of_trait.trait_ref.trait_def_id() else {
        return;
    };

    if cx.tcx.get_diagnostic_item(sym::Error) != Some(trait_def_id) {
        return;
    }

    // Resolve the `Self` type to its ADT definition. The impl may come from a
    // derive expansion (thiserror) rather than being hand-written, so this lint
    // deliberately does not skip `from_expansion`: the name it governs belongs to
    // the user's type, not to the generated impl.
    let self_ty = cx.tcx.type_of(item.owner_id.def_id).instantiate_identity();
    let Some(adt_def) = self_ty.ty_adt_def() else {
        return;
    };

    let did = adt_def.did();
    let Some(local_did) = did.as_local() else {
        return;
    };

    let name = cx.tcx.item_name(did);
    if name.as_str().ends_with("Error") {
        return;
    }

    // Anchor the diagnostic to the type's own HIR node so its lint level is read
    // from the type definition: `#[allow(error_type_naming)]` written on the type
    // is what silences it, not an attribute on the (possibly derived) impl.
    let hir_id = cx.tcx.local_def_id_to_hir_id(local_did);
    span_lint_hir_and_then(
        cx,
        ERROR_TYPE_NAMING,
        hir_id,
        cx.tcx.def_span(did),
        "type implementing `std::error::Error` is not named with an `Error` suffix",
        |diag| {
            diag.help(format!(
                "give `{name}` a name ending in `Error` so every type that implements `std::error::Error` \
                 is recognizable as one by name"
            ));
        },
    );
}
