pub fn run_overflow_fixture() -> i32 {
    crate::entrypoint::run(8)
}

#[cfg(target_os = "linux")]
pub fn run_kernel_worker() -> i32 {
    crate::linux::run_kernel_worker()
}

#[cfg(not(target_os = "linux"))]
pub fn run_kernel_worker() -> i32 {
    78
}
