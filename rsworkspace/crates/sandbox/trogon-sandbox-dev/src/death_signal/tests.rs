type Error = dyn std::error::Error;

/// Runs `arm_in_child(parent)` in a forked child that exits normally, so its coverage counters are written,
/// and returns the child's exit code.
fn exit_code_of_child_arming_for(parent: Option<u32>, block_sigterm: bool) -> Result<i32, Box<Error>> {
    // SAFETY: the child only calls libc functions and exits.
    let pid = unsafe { libc::fork() };
    (pid >= 0).then_some(()).ok_or_else(std::io::Error::last_os_error)?;
    if pid == 0 {
        if block_sigterm {
            // SAFETY: sigset_t is plain data; the calls only touch the child's own signal mask.
            unsafe {
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGTERM);
                libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
            }
        }
        // SAFETY: getppid takes no pointers.
        let parent = parent.unwrap_or_else(|| unsafe { libc::getppid() } as u32);
        let code = i32::from(super::arm_in_child(parent).is_err());
        // SAFETY: exit (not _exit) runs the handlers that flush coverage counters.
        unsafe { libc::exit(code) };
    }
    let mut status = 0;
    // SAFETY: waiting for our own child with a valid status pointer.
    let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
    assert_eq!(waited, pid);
    assert!(libc::WIFEXITED(status), "child did not exit normally");
    Ok(libc::WEXITSTATUS(status))
}

#[test]
fn arming_succeeds_while_the_supervisor_is_alive() -> Result<(), Box<Error>> {
    assert_eq!(exit_code_of_child_arming_for(None, false)?, 0);
    Ok(())
}

#[test]
fn arming_after_the_supervisor_died_raises_sigterm() -> Result<(), Box<Error>> {
    // A blocked SIGTERM is raised but stays pending, so the child can still report.
    assert_eq!(exit_code_of_child_arming_for(Some(1), true)?, 0);
    Ok(())
}
