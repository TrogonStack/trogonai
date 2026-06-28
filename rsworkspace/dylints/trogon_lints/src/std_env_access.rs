use clippy_utils::diagnostics::span_lint_and_then;
use rustc_hir::def::{DefKind, Res};
use rustc_hir::def_id::LOCAL_CRATE;
use rustc_hir::{Expr, ExprKind};
use rustc_lint::LateContext;

use crate::STD_ENV_ACCESS;

/// The `std::env` free functions that read a process environment variable. Each
/// has a `trogon_std::env::ReadEnv` equivalent that can be injected and faked,
/// so a direct call bypasses the abstraction.
const ENV_READERS: &[&str] = &["var", "var_os", "vars", "vars_os"];

#[derive(Default)]
pub(crate) struct StdEnvAccess;

impl StdEnvAccess {
    pub(crate) fn check_expr<'tcx>(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        // `trogon-std`'s `SystemEnv` is the single production `ReadEnv`
        // implementation; it is the one place allowed to reach `std::env`
        // directly so that every other crate can inject it.
        if cx.tcx.crate_name(LOCAL_CRATE).as_str() == "trogon_std" {
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
        if cx.tcx.crate_name(def_id.krate).as_str() != "std" {
            return;
        }
        if !ENV_READERS.contains(&cx.tcx.item_name(def_id).as_str()) {
            return;
        }
        if cx.tcx.item_name(cx.tcx.parent(def_id)).as_str() != "env" {
            return;
        }

        span_lint_and_then(
            cx,
            STD_ENV_ACCESS,
            func.span,
            "environment variable read directly through `std::env`",
            |diag| {
                diag.help("read it through an injected `trogon_std::env::ReadEnv` (`SystemEnv` in production, `InMemoryEnv` in tests) instead");
            },
        );
    }
}
