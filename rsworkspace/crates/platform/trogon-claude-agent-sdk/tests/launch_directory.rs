//! The launch directory is read when the binary loads, before `main`, as importing the SDK module reads it upstream.
//! This binary holds one test because it changes the process working directory.

use std::error::Error;

use trogon_claude_agent_sdk::adapters::capture_launch_directory;

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        std_env_access,
        reason = "The test moves the process working directory to show the launch directory was read before it."
    )
)]
#[test]
fn the_launch_directory_is_read_when_the_binary_loads() -> Result<(), Box<dyn Error>> {
    let start = std::env::current_dir()?;
    let elsewhere = tempfile::tempdir()?;
    std::env::set_current_dir(elsewhere.path())?;
    let captured = capture_launch_directory();
    std::env::set_current_dir(&start)?;

    assert_eq!(captured, Some(start));
    Ok(())
}
