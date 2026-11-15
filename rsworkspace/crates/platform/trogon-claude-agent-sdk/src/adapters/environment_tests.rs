use super::{CurrentDirectory, SystemEnvironment, capture_launch_directory};

#[test]
fn the_launch_directory_is_read_once_and_kept() {
    let first = capture_launch_directory();

    assert_eq!(first, SystemEnvironment.current_dir().ok());
    assert_eq!(capture_launch_directory(), first);
}
