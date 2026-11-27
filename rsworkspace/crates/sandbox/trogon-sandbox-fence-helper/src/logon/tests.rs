use super::*;
use crate::error::{FenceHelperError, Result};
use SeclogonState as S;
use windows::Win32::Foundation::ERROR_BROKEN_PIPE;
use windows::Win32::Security::RevertToSelf;
use windows::Win32::System::Pipes::{GetNamedPipeInfo, ImpersonateNamedPipeClient, PIPE_SERVER_END};
use windows::Win32::System::Services::{SERVICE_AUTO_START, SERVICE_DEMAND_START, SERVICE_STOPPED};
use windows::Win32::System::Threading::{CreateEventW, SetEvent};

#[test]
fn seclogon_disabled_only_when_not_running() {
    let stopped = Some(SERVICE_STOPPED);
    let running = Some(SERVICE_RUNNING);
    assert_eq!(classify_seclogon(SERVICE_DEMAND_START, stopped), S::Ok);
    assert_eq!(classify_seclogon(SERVICE_AUTO_START, running), S::Ok);
    assert_eq!(classify_seclogon(SERVICE_DISABLED, stopped), S::Disabled);
    assert_eq!(classify_seclogon(SERVICE_DISABLED, None), S::CannotRead);
    assert_eq!(classify_seclogon(SERVICE_DEMAND_START, None), S::Ok);
    // Disabled after it was started: still launches until the service stops.
    assert_eq!(classify_seclogon(SERVICE_DISABLED, running), S::Ok);
}

#[test]
fn logon_failure_is_seclogon_only_when_the_probe_agrees() {
    for service_disabled_error in [true, false] {
        assert!(logon_failure_is_seclogon(service_disabled_error, S::Disabled));
        assert!(logon_failure_is_seclogon(service_disabled_error, S::Missing));
        assert!(!logon_failure_is_seclogon(service_disabled_error, S::Ok));
    }
    assert!(logon_failure_is_seclogon(true, S::CannotRead));
    assert!(!logon_failure_is_seclogon(false, S::CannotRead));
}

#[test]
fn seclogon_state_json_values() -> Result<(), Box<dyn std::error::Error>> {
    let j = |s: SeclogonState| serde_json::to_value(s);
    assert_eq!(j(SeclogonState::Ok)?, "ok");
    assert_eq!(j(SeclogonState::Disabled)?, "disabled");
    assert_eq!(j(SeclogonState::Missing)?, "missing");
    assert_eq!(j(SeclogonState::CannotRead)?, "cannot-read");
    Ok(())
}

#[test]
fn seclogon_disabled_error_state_and_message() {
    let d = SeclogonDisabled { missing: false };
    assert_eq!(d.state(), SeclogonState::Disabled);
    assert!(d.to_string().contains("startup type to Manual"));
    let m = SeclogonDisabled { missing: true };
    assert_eq!(m.state(), SeclogonState::Missing);
    assert!(m.to_string().contains("not present"));
}

/// The command ends with its session: whichever of the two ends
/// first decides, the command when both have, and a holder that
/// cannot be waited on is a holder that is gone.
#[test]
fn the_wait_ends_with_the_child_or_with_the_holder() -> Result<(), Box<dyn std::error::Error>> {
    let event = || unsafe { CreateEventW(None, true, false, None) }.map(crate::util::OwnedHandle);
    let (child, holder) = (event()?, event()?);
    unsafe { SetEvent(holder.raw()) }?;
    assert_eq!(
        super::wait_for_child_or_holder(child.raw(), Some(holder.raw())),
        super::Ended::HolderGone
    );
    unsafe { SetEvent(child.raw()) }?;
    assert_eq!(
        super::wait_for_child_or_holder(child.raw(), Some(holder.raw())),
        super::Ended::Child
    );
    let holder = event()?;
    assert_eq!(
        super::wait_for_child_or_holder(child.raw(), Some(holder.raw())),
        super::Ended::Child
    );
    assert_eq!(super::wait_for_child_or_holder(child.raw(), None), super::Ended::Child);
    let running = event()?;
    let closed = windows::Win32::Foundation::HANDLE::default();
    assert_eq!(
        super::wait_for_child_or_holder(running.raw(), Some(closed)),
        super::Ended::HolderGone
    );
    let (child, holder) = (event()?, event()?);
    let signal = holder.raw().0 as isize;
    let t = std::thread::spawn(move || -> Result<(), String> {
        std::thread::sleep(std::time::Duration::from_millis(200));
        unsafe { SetEvent(windows::Win32::Foundation::HANDLE(signal as *mut _)) }.map_err(|e| e.to_string())?;
        Ok(())
    });
    assert_eq!(
        super::wait_for_child_or_holder(child.raw(), Some(holder.raw())),
        super::Ended::HolderGone
    );
    t.join().map_err(|_| "the signalling thread panicked")??;
    Ok(())
}

#[test]
fn stdin_pipe_name_shape() {
    let n = stdin_pipe_name(1234, &[0xab; 16]);
    assert_eq!(n, r"\\.\pipe\fence-helper-stdin-1234-abababababababababababababababab");
    let mut r = [0u8; 16];
    r[15] = 0x0f;
    assert!(stdin_pipe_name(1, &r).ends_with("-0000000000000000000000000000000f"));
    assert_ne!(stdin_pipe_name(1, &[0; 16]), stdin_pipe_name(1, &r));
}

#[test]
fn stdin_pipe_flags() {
    assert_eq!(STDIN_PIPE_OPEN_MODE.0 & 0x3, 0x2);
    assert_ne!(STDIN_PIPE_OPEN_MODE.0 & 0x0008_0000, 0);
    assert_eq!(STDIN_PIPE_OPEN_MODE.0 & 0x4000_0000, 0);
    assert_eq!(STDIN_PIPE_MODE.0, 0x8);
    assert_eq!(STDIN_PIPE_MAX_INSTANCES, 1);
    // GENERIC_READ | FILE_WRITE_ATTRIBUTES; SQOS present at level ANONYMOUS.
    assert_eq!(STDIN_CLIENT_ACCESS, 0x8000_0100);
    assert_eq!(STDIN_CLIENT_FLAGS.0, 0x0010_0000);
}

/// The runner's end reads like an anonymous pipe and cannot impersonate.
#[test]
fn stdin_pipe_runner_end_is_a_client_end() -> Result<(), Box<dyn std::error::Error>> {
    let name = fresh_stdin_pipe_name()?;
    let pipe = make_stdin_pipe_named(&name)?;

    // Read first: until data has been read through it, even a server end
    // is refused impersonation.
    let payload = b"spec-bytes";
    let mut wrote = 0u32;
    unsafe { WriteFile(pipe.broker.raw(), Some(payload), Some(&mut wrote), None) }?;
    assert_eq!(wrote as usize, payload.len());
    let mut buf = [0u8; 64];
    let mut read = 0u32;
    unsafe { ReadFile(pipe.runner.raw(), Some(&mut buf), Some(&mut read), None) }?;
    assert_eq!(&buf[..read as usize], payload);

    let imp = unsafe { ImpersonateNamedPipeClient(pipe.runner.raw()) };
    if imp.is_ok() {
        unsafe { RevertToSelf()? };
        return Err("ImpersonateNamedPipeClient succeeded on the runner's stdin end".into());
    }
    let server_end = |h: HANDLE| {
        let mut flags = NAMED_PIPE_MODE(0);
        unsafe { GetNamedPipeInfo(h, Some(&mut flags), None, None, None) }.map(|()| flags.0 & PIPE_SERVER_END.0 != 0)
    };
    assert!(
        !server_end(pipe.runner.raw())?,
        "the runner's stdin end is the pipe's SERVER end"
    );
    // The write-only server end may not be queryable; when it is, it must
    // read as the server end, or the check above proves nothing.
    if let Ok(is_server) = server_end(pipe.broker.raw()) {
        assert!(is_server, "the broker's end is not the pipe's server end");
    }

    let name_w = wstr(&name);
    let again = unsafe {
        CreateFileW(
            pcwstr(&name_w),
            STDIN_CLIENT_ACCESS,
            FILE_SHARE_MODE(0),
            None,
            OPEN_EXISTING,
            STDIN_CLIENT_FLAGS,
            None,
        )
    };
    let busy = again.err().ok_or("a second client connected to the stdin pipe")?;
    assert_eq!(busy.code(), ERROR_PIPE_BUSY.to_hresult());
    assert!(is_pipe_busy(
        &FenceHelperError::from(busy).context("CreateFileW(runner stdin client end)")
    ));
    assert!(
        make_stdin_pipe_named(&name).is_err(),
        "a second server was created under the stdin pipe's name"
    );

    // ERROR_BROKEN_PIPE is what a reader treats as EOF.
    drop(pipe.broker);
    let eof = unsafe { ReadFile(pipe.runner.raw(), Some(&mut buf), Some(&mut read), None) };
    assert_eq!(
        eof.err().ok_or("expected an error")?.code(),
        ERROR_BROKEN_PIPE.to_hresult(),
        "reader must see ERROR_BROKEN_PIPE after the writer closes"
    );
    Ok(())
}

#[test]
fn a_busy_stdin_pipe_is_retried_once() -> Result<(), Box<dyn std::error::Error>> {
    let busy = || FenceHelperError::from(windows::core::Error::from(ERROR_PIPE_BUSY));
    let other = || FenceHelperError::from(windows::core::Error::from(ERROR_BROKEN_PIPE));
    assert!(is_pipe_busy(&busy().context("connect")));
    assert!(!is_pipe_busy(&other().context("connect")));
    assert!(!is_pipe_busy(&FenceHelperError::msg("not a system error".to_string())));

    let mut calls = 0;
    let r = once_more_if_pipe_busy(|| {
        calls += 1;
        if calls == 1 { Err(busy()) } else { Ok(calls) }
    });
    assert_eq!((r?, calls), (2, 2));

    let mut calls = 0;
    let r: Result<()> = once_more_if_pipe_busy(|| {
        calls += 1;
        Err(busy())
    });
    assert!(r.is_err_and(|e| is_pipe_busy(&e)));
    assert_eq!(calls, 2);

    let mut calls = 0;
    let r: Result<()> = once_more_if_pipe_busy(|| {
        calls += 1;
        Err(other())
    });
    assert!(r.is_err());
    assert_eq!(calls, 1);
    Ok(())
}

#[test]
fn drive_letter_root() {
    assert_eq!(cwd_drive_root(r"Z:\work\repo"), Some(r"Z:\".into()));
    assert_eq!(cwd_drive_root("c:/work"), Some(r"C:\".into()));
    assert_eq!(cwd_drive_root("relative"), None);
}

#[test]
fn unc_root() {
    assert_eq!(cwd_drive_root(r"\\srv\share\dir\f"), Some(r"\\srv\share\".into()),);
    assert_eq!(cwd_drive_root(r"\\srv\share"), Some(r"\\srv\share\".into()));
    // server-only (no share) is not a valid UNC root
    assert_eq!(cwd_drive_root(r"\\srv"), None);
}

#[test]
fn device_and_extended_prefix_skipped() {
    // `\\?\` / `\\.\` are extended/device prefixes, not UNC -
    // left to the CPWLW post-check.
    assert_eq!(cwd_drive_root(r"\\?\C:\x"), None);
    assert_eq!(cwd_drive_root(r"\\.\pipe\x"), None);
}
