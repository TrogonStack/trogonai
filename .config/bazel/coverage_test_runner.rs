fn main() {
    let child = std::env::args_os().nth(1).expect("child executable");
    let status = std::process::Command::new(child).status().expect("run child");
    assert!(status.success());
}
