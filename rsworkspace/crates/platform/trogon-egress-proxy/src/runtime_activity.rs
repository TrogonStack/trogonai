use tokio_util::sync::CancellationToken;

use crate::Readiness;

pub(crate) struct RuntimeActivity {
    readiness: Readiness,
}

impl Readiness {
    pub(crate) fn running(&self, shutdown: CancellationToken) -> RuntimeActivity {
        self.set_running(shutdown);
        RuntimeActivity::new(self.clone())
    }
}

impl RuntimeActivity {
    pub(crate) fn new(readiness: Readiness) -> Self {
        Self { readiness }
    }
}

impl Drop for RuntimeActivity {
    fn drop(&mut self) {
        self.readiness.stopped();
    }
}
