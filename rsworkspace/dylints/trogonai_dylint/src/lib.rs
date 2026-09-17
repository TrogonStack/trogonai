#![feature(rustc_private)]

extern crate rustc_lint;
extern crate rustc_session;

use rustc_lint::LintStore;

dylint_linting::dylint_library!();

#[unsafe(no_mangle)]
pub fn register_lints(sess: &rustc_session::Session, _lint_store: &mut LintStore) {
    dylint_linting::init_config(sess);
}
