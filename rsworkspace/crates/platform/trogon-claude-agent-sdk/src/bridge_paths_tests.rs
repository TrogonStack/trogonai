use super::*;

#[test]
fn folds_path_text_the_way_the_path_module_does() {
    assert_eq!(dirname("/a/b/"), "/a");
    assert_eq!(dirname("/a"), "/");
    assert_eq!(dirname("/"), "/");
    assert_eq!(dirname("a"), ".");
    assert_eq!(dirname("a/b"), "a");
    assert_eq!(join("", "x"), "x");
    assert_eq!(join(".", "x"), "x");
    assert_eq!(join("/", "x"), "/x");
    assert_eq!(join("/a/", "x"), "/a/x");
    assert_eq!(resolve("/a/b", "../c"), "/a/c");
    assert_eq!(resolve("/a", "/x/./y"), "/x/y");
    assert_eq!(resolve("/a/b", ".."), "/a");
    assert_eq!(split_runs("a//b/"), ["a", "b", ""]);
    assert_eq!(split_runs(""), [""]);
}

#[test]
fn reads_automount_comments_like_the_upstream_pattern() {
    assert_eq!(without_comment("/home x # note"), "/home x ");
    assert_eq!(without_comment("/home x"), "/home x");
    assert_eq!(without_comment("# a\r"), "# a\r");
    assert!(lists_home("/net -hosts\n/home auto_home\n"));
    assert!(lists_home("  /HOME\tmap # c"));
    assert!(!lists_home("/home\n"));
    assert!(!lists_home("#/home map\n"));
}

#[test]
fn strips_the_data_volume_prefix_only_for_known_roots() {
    assert_eq!(data_volume("/System/Volumes/Data/home/u"), "/home/u");
    assert_eq!(data_volume("/system/volumes/DATA/usr/local/x"), "/usr/local/x");
    assert_eq!(data_volume("/System/Volumes/Data/usr"), "/System/Volumes/Data/usr");
    assert_eq!(data_volume("/System/Volumes/Data/zzz/x"), "/System/Volumes/Data/zzz/x");
    assert_eq!(data_volume("/home/u"), "/home/u");
}

#[test]
fn shares_one_guard_for_the_host() {
    assert!(Arc::ptr_eq(&PathGuard::host(), &PathGuard::host()));
    assert_eq!(format!("{:?}", PathGuard::host()), "PathGuard { .. }");
}
