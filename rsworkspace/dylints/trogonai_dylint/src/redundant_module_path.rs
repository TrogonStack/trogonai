use std::path::{Path, PathBuf};

use clippy_utils::diagnostics::span_lint_and_then;
use rustc_ast::{Inline, Item, ItemKind, ModKind};
use rustc_lint::{EarlyContext, EarlyLintPass, LintContext};
use rustc_span::{FileName, RealFileName, sym};

use crate::REDUNDANT_MODULE_PATH;

/// `#[path]` is consumed during expansion and never reaches HIR, so this must
/// run as an early (AST) pass to see the attribute at all.
pub(crate) struct RedundantModulePath;

impl EarlyLintPass for RedundantModulePath {
    fn check_item(&mut self, cx: &EarlyContext<'_>, item: &Item) {
        if item.span.from_expansion() {
            return;
        }

        // A file-backed module (`mod foo;`) is `Loaded` with `Inline::No`; the
        // body span covers the separate file it was read from. Inline `mod foo
        // { ... }` and modules that failed to parse are not our concern.
        let ItemKind::Mod(_, ident, ModKind::Loaded(_, Inline::No { had_parse_error: Ok(()) }, spans)) =
            &item.kind
        else {
            return;
        };

        let Some(path_attr) = item.attrs.iter().find(|attr| attr.has_name(sym::path)) else {
            return;
        };

        let source_map = cx.sess().source_map();
        let decl_file = source_map.lookup_char_pos(item.span.lo()).file;
        let body_file = source_map.lookup_char_pos(spans.inner_span.lo()).file;

        let (Some(decl_path), Some(body_path)) =
            (real_path(&decl_file.name), real_path(&body_file.name))
        else {
            return;
        };

        // The crate root owns its own directory regardless of its file name, so
        // its children resolve as siblings rather than into a subdirectory.
        let decl_is_crate_root = cx
            .sess()
            .local_crate_source_file()
            .and_then(RealFileName::into_local_path)
            .is_some_and(|root| root == decl_path);

        // The attribute is only redundant when the file it points at is exactly
        // the one `mod foo;` would have resolved to on its own. A `#[path]`
        // aimed anywhere else is load-bearing and must stay.
        let Some(defaults) = default_module_paths(&decl_path, ident.as_str(), decl_is_crate_root)
        else {
            return;
        };
        if !defaults.contains(&body_path) {
            return;
        }

        span_lint_and_then(
            cx,
            REDUNDANT_MODULE_PATH,
            path_attr.span,
            format!("redundant `#[path]` on module `{ident}`"),
            |diag| {
                diag.help(format!(
                    "remove the attribute; `mod {ident};` already resolves to this file"
                ));
            },
        );
    }
}

fn real_path(name: &FileName) -> Option<PathBuf> {
    match name {
        FileName::Real(real) => real.local_path().map(Path::to_path_buf),
        _ => None,
    }
}

/// The files `mod {module};` resolves to by default when declared inside
/// `decl_path`: `{module}.rs` and `{module}/mod.rs`, rooted at the directory
/// the declaring file owns. A directory-owning file (the crate root, or a
/// `mod.rs`/`lib.rs`/`main.rs`) owns its own directory; any other file `foo.rs`
/// owns `foo/`.
fn default_module_paths(
    decl_path: &Path,
    module: &str,
    decl_is_crate_root: bool,
) -> Option<[PathBuf; 2]> {
    let dir = decl_path.parent()?;
    let stem = decl_path.file_stem()?.to_str()?;
    let owner_dir = if decl_is_crate_root || matches!(stem, "mod" | "lib" | "main") {
        dir.to_path_buf()
    } else {
        dir.join(stem)
    };
    Some([
        owner_dir.join(format!("{module}.rs")),
        owner_dir.join(module).join("mod.rs"),
    ])
}
