use super::*;
use crate::linux::tests::{TestResult, isolated, refuse_stdout_frame_write};

struct StdoutGuard(OwnedFd);

impl StdoutGuard {
    fn redirect(descriptor: RawFd) -> Result<Self, std::io::Error> {
        let saved = unsafe { libc::fcntl(libc::STDOUT_FILENO, libc::F_DUPFD_CLOEXEC, 10) };
        if saved < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let saved = unsafe { OwnedFd::from_raw_fd(saved) };
        if unsafe { libc::dup2(descriptor, libc::STDOUT_FILENO) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self(saved))
    }
}

impl Drop for StdoutGuard {
    fn drop(&mut self) {
        unsafe {
            libc::dup2(self.0.as_raw_fd(), libc::STDOUT_FILENO);
        }
    }
}

pub(in crate::linux) fn worker_control_queue_refuses_overflow_without_dropping_retained_frames() -> TestResult {
    let mut writer = Writer::new();
    let execution = ExecutionId::new("control-bound")?;
    let event = Event::Status {
        execution_id: execution,
        phase: Phase::Held,
    };
    let bytes = encode_event(&event)?.len();
    for _ in 0..QUEUED_CONTROL_LIMIT / bytes {
        writer.queue(event.clone())?;
    }
    let retained = writer.pending.len();
    let queued = writer.control_bytes;
    assert_eq!(writer.queue(event), Err(ErrorCode::IoFailure));
    assert_eq!(writer.pending.len(), retained);
    assert_eq!(writer.control_bytes, queued);
    assert!(queued <= QUEUED_CONTROL_LIMIT);
    assert!(!writer.closed);
    Ok(())
}

#[test]
fn control_queue_refuses_overflow_without_dropping_retained_frames() -> TestResult {
    worker_control_queue_refuses_overflow_without_dropping_retained_frames()
}

pub(in crate::linux) fn worker_permanent_output_failure_closes_the_writer_and_releases_queue_accounting() -> TestResult
{
    isolated(
        "linux::writer::tests::permanent_output_failure_closes_the_writer_and_releases_queue_accounting",
        || {
            let (read, _write) = pipe()?;
            let _stdout = StdoutGuard::redirect(read.as_raw_fd())?;
            let mut writer = Writer::new();
            let execution = ExecutionId::new("closed-output")?;
            writer.queue(Event::Status {
                execution_id: execution.clone(),
                phase: Phase::Held,
            })?;
            writer.queue(Event::Output {
                execution_id: execution.clone(),
                stream: Stream::Stdout,
                data: vec![1],
            })?;
            assert!(writer.control_bytes > 0 && writer.data_bytes > 0);
            writer.flush();
            assert!(writer.closed);
            assert!(writer.pending.is_empty());
            assert_eq!(writer.control_bytes, 0);
            assert_eq!(writer.data_bytes, 0);
            assert_eq!(
                writer.queue(Event::Status {
                    execution_id: execution.clone(),
                    phase: Phase::Absent,
                }),
                Err(ErrorCode::IoFailure)
            );
            for stream in [Stream::Stdout, Stream::Stderr] {
                writer.queue(Event::Output {
                    execution_id: execution.clone(),
                    stream,
                    data: vec![2],
                })?;
            }
            assert!(writer.stdout_truncated && writer.stderr_truncated);
            assert!(writer.pending.is_empty());
            Ok(())
        },
    )
}

#[test]
fn permanent_output_failure_closes_the_writer_and_releases_queue_accounting() -> TestResult {
    worker_permanent_output_failure_closes_the_writer_and_releases_queue_accounting()
}

pub(in crate::linux) fn worker_a_full_nonblocking_pipe_retains_unsent_frames_and_queue_accounting() -> TestResult {
    isolated(
        "linux::writer::tests::a_full_nonblocking_pipe_retains_unsent_frames_and_queue_accounting",
        || {
            let (_read, write) = pipe()?;
            nonblocking(write.as_raw_fd())?;
            let padding = [0_u8; 8192];
            loop {
                let written = unsafe { libc::write(write.as_raw_fd(), padding.as_ptr().cast(), padding.len()) };
                if written < 0 {
                    assert_eq!(errno(), libc::EAGAIN);
                    break;
                }
            }
            let _stdout = StdoutGuard::redirect(write.as_raw_fd())?;
            let mut writer = Writer::new();
            writer.queue(Event::Status {
                execution_id: ExecutionId::new("backpressure")?,
                phase: Phase::Held,
            })?;
            let queued = writer.control_bytes;
            writer.flush();
            assert!(!writer.closed);
            assert_eq!(writer.pending.len(), 1);
            assert_eq!(writer.control_bytes, queued);
            assert_eq!(writer.pending.front().map(|frame| frame.offset), Some(0));
            Ok(())
        },
    )
}

#[test]
fn a_full_nonblocking_pipe_retains_unsent_frames_and_queue_accounting() -> TestResult {
    worker_a_full_nonblocking_pipe_retains_unsent_frames_and_queue_accounting()
}

pub(in crate::linux) fn worker_kernel_zero_write_retains_the_nonempty_frame_without_progress() -> TestResult {
    isolated(
        "linux::writer::tests::kernel_zero_write_retains_the_nonempty_frame_without_progress",
        || {
            let mut writer = Writer::new();
            writer.queue(Event::Output {
                execution_id: ExecutionId::new("zero-progress")?,
                stream: Stream::Stdout,
                data: vec![1; 3000],
            })?;
            let queued = writer.data_bytes;
            assert!(queued > 3000);
            refuse_stdout_frame_write(queued)?;
            writer.flush();
            assert!(!writer.closed);
            assert_eq!(writer.data_bytes, queued);
            assert_eq!(writer.pending.len(), 1);
            assert_eq!(writer.pending.front().map(|frame| frame.offset), Some(0));
            Ok(())
        },
    )
}

#[test]
fn kernel_zero_write_retains_the_nonempty_frame_without_progress() -> TestResult {
    worker_kernel_zero_write_retains_the_nonempty_frame_without_progress()
}

pub(in crate::linux) fn dispatch_worker(name: &str) -> Option<crate::linux::tests::TestResult> {
    match name {
        "linux::writer::tests::control_queue_refuses_overflow_without_dropping_retained_frames" => {
            Some(worker_control_queue_refuses_overflow_without_dropping_retained_frames())
        }
        "linux::writer::tests::permanent_output_failure_closes_the_writer_and_releases_queue_accounting" => {
            Some(worker_permanent_output_failure_closes_the_writer_and_releases_queue_accounting())
        }
        "linux::writer::tests::a_full_nonblocking_pipe_retains_unsent_frames_and_queue_accounting" => {
            Some(worker_a_full_nonblocking_pipe_retains_unsent_frames_and_queue_accounting())
        }
        "linux::writer::tests::kernel_zero_write_retains_the_nonempty_frame_without_progress" => {
            Some(worker_kernel_zero_write_retains_the_nonempty_frame_without_progress())
        }
        _ => None,
    }
}
