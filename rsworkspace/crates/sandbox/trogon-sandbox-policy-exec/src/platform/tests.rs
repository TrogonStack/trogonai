use super::*;

#[test]
fn an_explicit_marker_gives_its_number() {
    let text = "Linux version 5.15.90.1-microsoft-standard-WSL2 (root@x)";
    assert_eq!(WslVersion::from_proc_version(text).map(WslVersion::get), Some(2));
    assert_eq!(WslVersion::from_proc_version("x wsl3 y").map(WslVersion::get), Some(3));
}

#[test]
fn microsoft_without_a_marker_is_wsl1() {
    let version = WslVersion::from_proc_version("Linux version 4.4.0-19041-Microsoft");
    assert_eq!(version.map(WslVersion::get), Some(1));
    assert!(!version.is_some_and(WslVersion::is_supported));
}

#[test]
fn a_plain_kernel_is_not_wsl() {
    assert_eq!(WslVersion::from_proc_version("Linux version 6.1.0-generic"), None);
}

#[test]
fn a_marker_without_digits_does_not_count() {
    assert_eq!(WslVersion::from_proc_version("wsl is great"), None);
    assert_eq!(
        WslVersion::from_proc_version("wsl and Microsoft").map(WslVersion::get),
        Some(1)
    );
}

#[test]
fn wsl2_and_later_are_supported() {
    assert!(WslVersion(2).is_supported());
    assert!(!WslVersion(1).is_supported());
}

#[cfg(not(target_os = "linux"))]
#[test]
fn off_linux_there_is_no_wsl() {
    assert_eq!(WslVersion::detect(), None);
    assert!(is_supported());
}
