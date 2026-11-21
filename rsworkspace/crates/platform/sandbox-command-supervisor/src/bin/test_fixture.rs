#[cfg(target_os = "linux")]
#[path = "test_fixture/fixture.rs"]
mod fixture;

fn main() {
    #[cfg(target_os = "linux")]
    std::process::exit(fixture::run());
    #[cfg(not(target_os = "linux"))]
    std::process::exit(78);
}
