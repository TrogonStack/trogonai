use tokio::sync::watch;

use crate::Readiness;

#[derive(Clone, Debug)]
pub struct Health {
    readiness: watch::Sender<Readiness>,
}

impl Default for Health {
    fn default() -> Self {
        let (readiness, _) = watch::channel(Readiness::Ready);
        Self { readiness }
    }
}

impl Health {
    pub fn readiness(&self) -> Readiness {
        *self.readiness.borrow()
    }

    pub fn begin_shutdown(&self) {
        self.readiness.send_replace(Readiness::Draining);
    }
}
