use std::{ffi::OsString, path::PathBuf};

use super::flag_is_true;
use crate::test_support::FixedEnvironment;

const NAME: &str = "CLAUDE_CODE_DISABLE_ADMIN_ENV_UNION";

fn host(value: &str) -> FixedEnvironment {
    FixedEnvironment {
        directory: PathBuf::from("/work"),
        variables: [(OsString::from(NAME), OsString::from(value))].into_iter().collect(),
    }
}

#[test]
fn a_flag_trims_like_javascript() {
    for value in ["1", " Yes ", "\u{feff}1\u{feff}", "\u{feff}true"] {
        assert!(flag_is_true(&host(value), NAME), "{value:?}");
    }
    for value in ["", "0", "\u{85}1\u{85}", "\u{85}true"] {
        assert!(!flag_is_true(&host(value), NAME), "{value:?}");
    }
    assert!(!flag_is_true(&host("1"), "ANOTHER_VARIABLE"));
}
