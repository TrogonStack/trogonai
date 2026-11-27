//! The thread every child is forked from. `PR_SET_PDEATHSIG` follows the
//! thread that forked the child, and a runtime may retire the thread a caller
//! happens to run on, so forking from one thread that never exits keeps the
//! death signal tied to the process.

use std::io;
use std::sync::OnceLock;
use std::sync::mpsc::{SyncSender, sync_channel};

type Job = Box<dyn FnOnce() + Send>;

static JOBS: OnceLock<Option<SyncSender<Job>>> = OnceLock::new();

fn start() -> Option<SyncSender<Job>> {
    // Every caller waits for its job to finish, so at most one job per caller is ever queued.
    let (sender, receiver) = sync_channel::<Job>(1);
    let spawned = std::thread::Builder::new()
        .name("trogon-sandbox-dev-spawner".into())
        .spawn(move || receiver.into_iter().for_each(|job| job()));
    spawned.ok().map(|_| sender)
}

pub(crate) fn run<T: Send + 'static>(job: impl FnOnce() -> T + Send + 'static) -> io::Result<T> {
    let jobs = JOBS
        .get_or_init(start)
        .as_ref()
        .ok_or_else(|| io::Error::other("the spawner thread could not start"))?;
    let (done_sender, done_receiver) = sync_channel(1);
    jobs.send(Box::new(move || drop(done_sender.send(job()))))
        .map_err(|_| io::Error::other("the spawner thread exited"))?;
    done_receiver
        .recv()
        .map_err(|_| io::Error::other("the spawner thread exited"))
}
