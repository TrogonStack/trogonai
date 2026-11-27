use super::*;

#[test]
fn path_prefix_matches_exact_match() {
    assert!(path_prefix_matches("/srv/workspaces", "/srv/workspaces"));
}

#[test]
fn path_prefix_matches_child_path() {
    assert!(path_prefix_matches("/srv/workspaces", "/srv/workspaces/acme/repo"));
}

#[test]
fn path_prefix_matches_boundary_miss() {
    assert!(!path_prefix_matches("/foo", "/foobar"));
}

#[test]
fn path_prefix_matches_empty_prefix_false() {
    assert!(!path_prefix_matches("", "/srv/workspaces"));
}

#[test]
fn path_prefix_matches_empty_path_false() {
    assert!(!path_prefix_matches("/srv/workspaces", ""));
}

#[test]
fn path_prefix_matches_non_absolute_prefix_false() {
    assert!(!path_prefix_matches("srv/workspaces", "/srv/workspaces/repo"));
}
