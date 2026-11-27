use crate::k8s_ops::image_user_is_non_root_uid;

#[test]
fn only_a_non_zero_numeric_image_user_counts_as_non_root() {
    assert!(image_user_is_non_root_uid(Some("1000")));
    assert!(image_user_is_non_root_uid(Some("1000:2000")));
    assert!(!image_user_is_non_root_uid(None));
    assert!(!image_user_is_non_root_uid(Some("")));
    assert!(!image_user_is_non_root_uid(Some("0")));
    assert!(!image_user_is_non_root_uid(Some("0:0")));
    assert!(!image_user_is_non_root_uid(Some("root")));
    assert!(!image_user_is_non_root_uid(Some("node")));
}
