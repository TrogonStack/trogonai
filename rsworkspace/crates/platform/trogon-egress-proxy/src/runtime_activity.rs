use crate::Readiness;

pub(crate) struct RuntimeActivity {
    readiness: Readiness,
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
