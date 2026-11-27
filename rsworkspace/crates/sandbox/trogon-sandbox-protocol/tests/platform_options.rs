use trogon_sandbox_contract::{MachServiceName, MandatoryDenySearchDepth};
use trogon_sandbox_protocol::{
    MachServiceMatch, mach_service_matcher, search_depth_reaches_directory, search_depth_reaches_file,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn files_reach_one_level_deeper_than_directories() -> TestResult {
    let depth = MandatoryDenySearchDepth::new(3)?;
    assert!(search_depth_reaches_file(depth, 3) && !search_depth_reaches_file(depth, 4));
    assert!(search_depth_reaches_directory(depth, 2) && !search_depth_reaches_directory(depth, 3));
    Ok(())
}

#[test]
fn a_trailing_wildcard_is_a_prefix_and_anything_else_is_exact() -> TestResult {
    let exact = MachServiceName::new("com.example.service")?;
    assert_eq!(
        mach_service_matcher(&exact),
        MachServiceMatch::Exact("com.example.service")
    );
    let prefix = MachServiceName::new("com.example.*")?;
    assert_eq!(mach_service_matcher(&prefix), MachServiceMatch::Prefix("com.example."));
    assert_eq!(
        mach_service_matcher(&MachServiceName::new("*")?),
        MachServiceMatch::Prefix("")
    );
    Ok(())
}
