//! A parent proxy named by the environment must parse or be refused.

use super::*;

#[test]
fn an_unparseable_environment_proxy_is_refused_not_ignored() {
    for (name, value) in [("HTTP_PROXY", "not a url"), ("HTTPS_PROXY", "ftp://p:1")] {
        let result = network_options_with(r#""parentProxy": {}"#, &OneVar(name, value));
        assert!(result.is_err(), "{name}={value} was ignored");
    }
}
