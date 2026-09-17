use rustc_hir::def::{DefKind, Res};
use rustc_hir::{Expr, ExprKind, QPath};
use rustc_lint::LateContext;
use rustc_middle::ty;
use rustc_span::Symbol;

/// The `Kind` variant named by the last argument of a
/// `tracing_core::Metadata::new` call, or `None` when the call is something
/// else.
///
/// Every `tracing` callsite lowers to this one constructor:
/// `info_span!`/`span!` and `#[instrument]` pass `Kind::SPAN`, the event macros
/// (`info!`, `warn!`, `error!`, `debug!`, `trace!`, `event!`) pass
/// `Kind::EVENT`. Reading the kind back out is what lets a lint address one
/// family without also seeing the other.
pub(crate) fn metadata_new_kind<'tcx>(
    cx: &LateContext<'tcx>,
    callee: &'tcx Expr<'tcx>,
    args: &'tcx [Expr<'tcx>],
) -> Option<Symbol> {
    let ExprKind::Path(qpath) = &callee.kind else {
        return None;
    };
    let Res::Def(DefKind::AssocFn, did) = cx.qpath_res(qpath, callee.hir_id) else {
        return None;
    };
    if cx.tcx.item_name(did).as_str() != "new" {
        return None;
    }
    // Only an inherent or trait `impl` has a self type to ask for; a call that
    // resolves to a trait's own associated fn (`Trait::new`) has a trait as its
    // parent, and asking `type_of` for one is an ICE rather than a `None`.
    let parent = cx.tcx.parent(did);
    if !matches!(cx.tcx.def_kind(parent), DefKind::Impl { .. }) {
        return None;
    }
    let self_ty = cx.tcx.type_of(parent).instantiate_identity().peel_refs();
    let ty::Adt(adt, _) = self_ty.kind() else {
        return None;
    };
    let adt_did = adt.did();
    if cx.tcx.crate_name(adt_did.krate).as_str() != "tracing_core"
        || cx.tcx.item_name(adt_did).as_str() != "Metadata"
    {
        return None;
    }
    args.last().and_then(path_last_segment)
}

fn path_last_segment(expr: &Expr<'_>) -> Option<Symbol> {
    match &expr.kind {
        ExprKind::Path(QPath::Resolved(_, path)) => path.segments.last().map(|seg| seg.ident.name),
        ExprKind::Path(QPath::TypeRelative(_, seg)) => Some(seg.ident.name),
        _ => None,
    }
}
