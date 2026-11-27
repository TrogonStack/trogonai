/// Turns the return value of a libc call into an I/O result, reading `errno` only when the call failed.
pub(crate) fn syscall_result(result: libc::c_int) -> std::io::Result<()> {
    nix::errno::Errno::result(result)
        .map(drop)
        .map_err(std::io::Error::from)
}
